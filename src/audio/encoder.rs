//! Encodage et découpe des enregistrements.
//!
//! Deux formats de sortie réels sont proposés (voir
//! `settings::OutputFormatSetting`) :
//!
//! * **MP3** (par défaut) — encodage à la volée par LAME
//!   (`mp3lame-encoder`), en CBR, 16 kHz mono. Ce sont de **vraies trames
//!   MP3** avec en-têtes valides, jamais un WAV renommé : le fichier est
//!   lisible par n'importe quel lecteur et directement exploitable par une
//!   IA de transcription externe.
//! * **WAV** — PCM 16 bits non compressé (voir
//!   [`RecordingWavWriter`](super::wav_writer::RecordingWavWriter)).
//!
//! [`ChunkedRecorder`] encapsule ce choix et gère la **découpe à la volée**
//! (§5 du cahier des charges) : dès que le fichier courant atteint la taille
//! maximale demandée (moins une marge de sécurité), l'encodeur est vidé
//! proprement, le fichier est fermé, et un nouveau morceau
//! (`..._part2.mp3`, …) est ouvert **sans aucune perte audio** : les trames
//! suivantes y sont écrites directement.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::dsp::TARGET_SAMPLE_RATE;
use super::error::{AudioError, AudioResult};
use super::wav_writer::RecordingWavWriter;

/// Débit MP3 constant, adapté à la voix : 32 kbps ≈ 14 Mo par heure.
pub const MP3_BITRATE_KBPS: u16 = 32;

/// Marge de sécurité retirée de la taille maximale avant de basculer sur le
/// morceau suivant (§5 : « moins une marge de sécurité de 64 Ko »), pour ne
/// jamais dépasser la limite visée, même en tenant compte de la dernière
/// trame MP3 écrite.
const CHUNK_SAFETY_MARGIN_BYTES: u64 = 64 * 1024;

/// En-tête RIFF/WAVE standard (44 octets) d'un fichier PCM produit par
/// [`RecordingWavWriter`] — utilisé pour estimer la taille écrite sans
/// interroger le système de fichiers (voir `WavSink::bytes_written`).
const WAV_HEADER_BYTES: u64 = 44;

/// Format réel du fichier audio produit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingFormat {
    Mp3,
    Wav,
}

impl RecordingFormat {
    /// Extension de fichier (sans le point).
    pub fn extension(self) -> &'static str {
        match self {
            RecordingFormat::Mp3 => "mp3",
            RecordingFormat::Wav => "wav",
        }
    }
}

/// Résultat d'un enregistrement (ou d'un import) : tous les morceaux
/// produits (un seul si la découpe est désactivée ou si elle n'a pas été
/// déclenchée) et la durée totale d'audio réellement écrit.
#[derive(Debug, Clone)]
pub struct RecordingSummary {
    pub paths: Vec<PathBuf>,
    pub duration: Duration,
}

/// Convertit une taille de morceau exprimée en Mo en octets. `0` (ou une
/// valeur nulle) signifie « pas de découpe » → `None`.
pub fn chunk_bytes_from_mb(mb: u32) -> Option<u64> {
    if mb == 0 {
        None
    } else {
        Some(u64::from(mb) * 1024 * 1024)
    }
}

/// Un fichier de sortie unique, quel que soit le format.
enum Sink {
    Mp3(Mp3Sink),
    Wav(WavSink),
}

impl Sink {
    fn create(path: &Path, format: RecordingFormat) -> AudioResult<Self> {
        match format {
            RecordingFormat::Mp3 => Ok(Sink::Mp3(Mp3Sink::create(path)?)),
            RecordingFormat::Wav => Ok(Sink::Wav(WavSink::create(path)?)),
        }
    }

    fn write_pcm(&mut self, samples: &[i16]) -> AudioResult<()> {
        match self {
            Sink::Mp3(sink) => sink.write_pcm(samples),
            Sink::Wav(sink) => sink.write_pcm(samples),
        }
    }

    /// Nombre d'octets déjà écrits dans le fichier courant (en-têtes
    /// compris), pour décider de la découpe.
    fn bytes_written(&self) -> u64 {
        match self {
            Sink::Mp3(sink) => sink.bytes_written,
            Sink::Wav(sink) => sink.bytes_written(),
        }
    }

    /// Vide l'encodeur, ferme le fichier proprement et retourne son chemin.
    fn finalize(self) -> AudioResult<PathBuf> {
        match self {
            Sink::Mp3(sink) => sink.finalize(),
            Sink::Wav(sink) => sink.finalize(),
        }
    }
}

/// Encodeur MP3 réel (LAME), en CBR, mono 16 kHz.
struct Mp3Sink {
    path: PathBuf,
    file: BufWriter<File>,
    encoder: mp3lame_encoder::Encoder,
    /// Tampon de sortie réutilisé d'une trame à l'autre (évite une
    /// allocation par rappel audio).
    buffer: Vec<u8>,
    bytes_written: u64,
}

impl Mp3Sink {
    fn create(path: &Path) -> AudioResult<Self> {
        use mp3lame_encoder::{Bitrate, Builder, Quality};

        let mut builder = Builder::new()
            .ok_or_else(|| AudioError::Mp3Init("allocation de l'encodeur LAME impossible".into()))?;
        // `BuildError` n'expose pas de `Display` stable : on formate en
        // `Debug`, suffisant pour un diagnostic interne.
        builder
            .set_num_channels(1)
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;
        builder
            .set_sample_rate(TARGET_SAMPLE_RATE)
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;
        builder
            .set_brate(Bitrate::Kbps32)
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;
        builder
            .set_quality(Quality::Best)
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;
        // CBR : on n'écrit PAS de tag VBR/Info. La sortie se réduit donc à
        // des trames MP3 brutes — ce qui rend chaque morceau d'une découpe
        // autonome et valide, sans en-tête à réécrire.
        builder
            .set_to_write_vbr_tag(false)
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;
        let encoder = builder
            .build()
            .map_err(|e| AudioError::Mp3Init(format!("{e:?}")))?;

        let file = File::create(path)
            .map_err(|source| AudioError::Io { path: path.to_path_buf(), source })?;

        Ok(Self {
            path: path.to_path_buf(),
            file: BufWriter::new(file),
            encoder,
            buffer: Vec::new(),
            bytes_written: 0,
        })
    }

    fn write_pcm(&mut self, samples: &[i16]) -> AudioResult<()> {
        use mp3lame_encoder::{max_required_buffer_size, MonoPcm};

        if samples.is_empty() {
            return Ok(());
        }

        self.prepare_buffer(max_required_buffer_size(samples.len()));
        let encoded = self
            .encoder
            .encode(MonoPcm(samples), self.buffer.spare_capacity_mut())
            .map_err(|e| AudioError::Mp3Encode(format!("{e:?}")))?;
        // SAFETY: `encode` vient d'initialiser exactement `encoded` octets du
        // tampon (au plus sa capacité réservée).
        unsafe { self.buffer.set_len(encoded) };
        self.flush_buffer_to_file()
    }

    /// Vide l'encodeur (dernières trames + remplissage final), écrit le
    /// reste puis ferme le fichier. Indispensable pour que le dernier
    /// morceau soit un MP3 complet et lisible.
    fn finalize(mut self) -> AudioResult<PathBuf> {
        use mp3lame_encoder::FlushNoGap;

        // LAME documente qu'un flush final peut écrire jusqu'à 7200 octets :
        // on réserve large (16 Ko) pour ne jamais tronquer la dernière trame.
        self.prepare_buffer(16 * 1024);
        let encoded = self
            .encoder
            .flush::<FlushNoGap>(self.buffer.spare_capacity_mut())
            .map_err(|e| AudioError::Mp3Encode(format!("{e:?}")))?;
        unsafe { self.buffer.set_len(encoded) };
        self.flush_buffer_to_file()?;

        self.file
            .flush()
            .map_err(|source| AudioError::Io { path: self.path.clone(), source })?;
        Ok(self.path)
    }

    /// Vide le tampon et réserve au moins `needed` octets.
    fn prepare_buffer(&mut self, needed: usize) {
        self.buffer.clear();
        if self.buffer.capacity() < needed {
            self.buffer.reserve(needed - self.buffer.capacity());
        }
    }

    fn flush_buffer_to_file(&mut self) -> AudioResult<()> {
        if let Err(source) = self.file.write_all(&self.buffer) {
            return Err(AudioError::Io { path: self.path.clone(), source });
        }
        self.bytes_written += self.buffer.len() as u64;
        Ok(())
    }
}

/// Écriture WAV PCM 16 bits non compressée (délègue à
/// [`RecordingWavWriter`]).
struct WavSink {
    writer: RecordingWavWriter,
}

impl WavSink {
    fn create(path: &Path) -> AudioResult<Self> {
        Ok(Self { writer: RecordingWavWriter::create(path)? })
    }

    fn write_pcm(&mut self, samples: &[i16]) -> AudioResult<()> {
        self.writer.write_samples(samples)
    }

    /// Taille estimée sans toucher au disque (l'écriture passe par un
    /// tampon interne) : en-tête RIFF + 2 octets par échantillon.
    fn bytes_written(&self) -> u64 {
        WAV_HEADER_BYTES + self.writer.samples_written() * 2
    }

    fn finalize(self) -> AudioResult<PathBuf> {
        self.writer.finalize()
    }
}

/// Enregistreur gérant la découpe automatique en plusieurs fichiers.
pub struct ChunkedRecorder {
    dir: PathBuf,
    stem: String,
    format: RecordingFormat,
    /// `Some(limite)` si la découpe est active, `None` pour un fichier unique.
    max_bytes: Option<u64>,
    part: u32,
    sink: Sink,
    /// Morceaux déjà terminés (avant le morceau courant).
    finished: Vec<PathBuf>,
    /// Total d'échantillons PCM écrits, pour calculer la durée.
    samples_written: u64,
}

impl ChunkedRecorder {
    /// Ouvre le premier morceau dans `dir`, sous la racine `stem` (voir
    /// `paths::timestamped_stem`). `max_bytes` fixe la taille maximale par
    /// morceau (`None` = fichier unique).
    pub fn create(
        dir: impl Into<PathBuf>,
        stem: impl Into<String>,
        format: RecordingFormat,
        max_bytes: Option<u64>,
    ) -> AudioResult<Self> {
        let dir = dir.into();
        let stem = stem.into();

        std::fs::create_dir_all(&dir)
            .map_err(|source| AudioError::Io { path: dir.clone(), source })?;

        let chunked = max_bytes.is_some();
        let first_path = part_path(&dir, &stem, format, chunked, 1);
        let sink = Sink::create(&first_path, format)?;

        Ok(Self {
            dir,
            stem,
            format,
            max_bytes,
            part: 1,
            sink,
            finished: Vec::new(),
            samples_written: 0,
        })
    }

    /// Durée d'audio réellement écrite jusqu'ici (silences ignorés par la
    /// grille anti-silence exclus, voir `capture`).
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples_written as f64 / f64::from(TARGET_SAMPLE_RATE))
    }

    /// Écrit une trame PCM (16 kHz mono) et bascule sur le morceau suivant
    /// si la taille maximale est atteinte.
    ///
    /// La bascule est évaluée **avant** l'écriture : le nouveau morceau est
    /// donc immédiatement alimenté par la trame courante. C'est essentiel —
    /// ouvrir le morceau suivant de façon anticipée laisserait, si
    /// l'enregistrement s'arrête juste après, un dernier fichier vide (un
    /// encodeur MP3 fraîchement initialisé ne produit rien tant qu'il n'a
    /// reçu aucune trame).
    pub fn write_pcm(&mut self, samples: &[i16]) -> AudioResult<()> {
        if samples.is_empty() {
            return Ok(());
        }

        if let Some(limit) = self.max_bytes {
            if self.sink.bytes_written() + CHUNK_SAFETY_MARGIN_BYTES >= limit {
                self.rotate()?;
            }
        }

        self.sink.write_pcm(samples)?;
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    /// Ferme le morceau courant et en ouvre un nouveau, sans perdre une
    /// seule trame.
    fn rotate(&mut self) -> AudioResult<()> {
        let next_part = self.part + 1;
        let next_path = part_path(
            &self.dir,
            &self.stem,
            self.format,
            self.max_bytes.is_some(),
            next_part,
        );
        // On ouvre le nouveau morceau AVANT de finaliser l'ancien : si la
        // création échoue (disque plein, etc.), on n'a rien fermé et la
        // trame suivante continuera simplement dans le fichier courant.
        let next_sink = Sink::create(&next_path, self.format)?;
        let previous = std::mem::replace(&mut self.sink, next_sink);
        self.finished.push(previous.finalize()?);
        self.part = next_part;
        Ok(())
    }

    /// Vide l'encodeur, ferme le dernier morceau et retourne tous les
    /// fichiers produits avec la durée totale.
    pub fn finalize(mut self) -> AudioResult<RecordingSummary> {
        let duration = self.duration();
        let last = self.sink.finalize()?;
        let mut paths = std::mem::take(&mut self.finished);

        // Garde-fou : un dernier morceau resté vide (aucune trame reçue
        // depuis la dernière bascule) est un artefact — on ne le conserve
        // pas, pour ne jamais produire de fichier MP3 de 0 octet.
        let last_is_empty = std::fs::metadata(&last)
            .map(|meta| meta.len() == 0)
            .unwrap_or(false);
        if last_is_empty && !paths.is_empty() {
            let _ = std::fs::remove_file(&last);
        } else {
            paths.push(last);
        }

        Ok(RecordingSummary { paths, duration })
    }
}

/// Chemin d'un morceau : `rec_..._partN.ext` si la découpe est active,
/// `rec_....ext` sinon (un fichier unique n'a pas besoin de suffixe).
fn part_path(
    dir: &Path,
    stem: &str,
    format: RecordingFormat,
    chunked: bool,
    part: u32,
) -> PathBuf {
    let name = if chunked {
        format!("{stem}_part{part}.{}", format.extension())
    } else {
        format!("{stem}.{}", format.extension())
    };
    dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_bytes_from_mb_zero_means_no_chunking() {
        assert_eq!(chunk_bytes_from_mb(0), None);
    }

    #[test]
    fn chunk_bytes_from_mb_is_in_mebibytes() {
        assert_eq!(chunk_bytes_from_mb(1), Some(1024 * 1024));
        assert_eq!(chunk_bytes_from_mb(25), Some(25 * 1024 * 1024));
    }

    #[test]
    fn part_path_omits_suffix_when_not_chunking() {
        let dir = Path::new("C:/tmp");
        assert_eq!(
            part_path(dir, "rec_x", RecordingFormat::Mp3, false, 1),
            PathBuf::from("C:/tmp/rec_x.mp3")
        );
        assert_eq!(
            part_path(dir, "rec_x", RecordingFormat::Mp3, true, 2),
            PathBuf::from("C:/tmp/rec_x_part2.mp3")
        );
    }

    #[test]
    fn format_extensions() {
        assert_eq!(RecordingFormat::Mp3.extension(), "mp3");
        assert_eq!(RecordingFormat::Wav.extension(), "wav");
    }

    #[test]
    fn single_file_mp3_starts_with_a_valid_frame_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let mut recorder = ChunkedRecorder::create(
            tmp.path(),
            "rec_test",
            RecordingFormat::Mp3,
            None,
        )
        .unwrap();

        // 1 seconde de silence à 16 kHz : assez pour produire des trames.
        recorder.write_pcm(&vec![0_i16; TARGET_SAMPLE_RATE as usize]).unwrap();
        let summary = recorder.finalize().unwrap();

        assert_eq!(summary.paths.len(), 1);
        let bytes = std::fs::read(&summary.paths[0]).unwrap();
        assert!(bytes.len() > 100, "fichier MP3 suspicieusement petit");
        // En CBR sans tag VBR, le fichier commence directement par une trame
        // MP3 : mot de synchronisation 0xFF 0xEx.
        assert_eq!(bytes[0], 0xFF, "mot de synchronisation MP3 absent");
        assert_eq!(bytes[1] & 0xE0, 0xE0, "mot de synchronisation MP3 invalide");
        assert!(summary.duration.as_secs_f64() > 0.9);
    }

    #[test]
    fn chunking_produces_several_valid_independent_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Limite volontairement minuscule (marge de 64 Ko + ~2 Ko utiles),
        // pour forcer plusieurs morceaux sur un signal court.
        let limit = CHUNK_SAFETY_MARGIN_BYTES + 2_000;
        let mut recorder =
            ChunkedRecorder::create(tmp.path(), "rec_chunk", RecordingFormat::Mp3, Some(limit))
                .unwrap();

        // 4 secondes de signal non silencieux (alternance d'amplitude).
        let mut block = vec![0_i16; TARGET_SAMPLE_RATE as usize];
        for (i, sample) in block.iter_mut().enumerate() {
            *sample = if i % 2 == 0 { 12_000 } else { -12_000 };
        }
        for _ in 0..4 {
            recorder.write_pcm(&block).unwrap();
        }
        let summary = recorder.finalize().unwrap();

        assert!(
            summary.paths.len() >= 2,
            "la découpe aurait dû produire plusieurs morceaux, obtenu {}",
            summary.paths.len()
        );
        for path in &summary.paths {
            assert!(path.is_file(), "morceau manquant : {}", path.display());
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(bytes[0], 0xFF, "morceau sans synchronisation MP3");
            assert_eq!(bytes[1] & 0xE0, 0xE0, "morceau MP3 invalide");
        }
    }

    #[test]
    fn chunking_respects_the_requested_limit() {
        let tmp = tempfile::tempdir().unwrap();
        // 100 Ko : à 32 kbps (≈4 Ko/s), la découpe se déclenche vers 9 s.
        let limit = 100 * 1024;
        let mut recorder =
            ChunkedRecorder::create(tmp.path(), "rec_limit", RecordingFormat::Mp3, Some(limit))
                .unwrap();

        let mut block = vec![0_i16; TARGET_SAMPLE_RATE as usize];
        for (i, sample) in block.iter_mut().enumerate() {
            *sample = if i % 2 == 0 { 9_000 } else { -9_000 };
        }
        for _ in 0..30 {
            recorder.write_pcm(&block).unwrap();
        }
        let summary = recorder.finalize().unwrap();

        assert!(summary.paths.len() >= 2, "la découpe n'a pas eu lieu");
        // Chaque morceau reste sous la limite (marge incluse)…
        for path in &summary.paths {
            let size = std::fs::metadata(path).unwrap().len();
            assert!(size <= limit, "morceau {size} octets > limite {limit}");
        }
        // …et l'audio total n'est pas perdu (30 s écrites).
        assert!(summary.duration.as_secs_f64() > 29.0);
    }

    #[test]
    fn wav_sink_writes_a_readable_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut recorder =
            ChunkedRecorder::create(tmp.path(), "rec_wav", RecordingFormat::Wav, None).unwrap();
        recorder.write_pcm(&vec![1_000_i16; 1_600]).unwrap();
        let summary = recorder.finalize().unwrap();

        let reader = hound::WavReader::open(&summary.paths[0]).unwrap();
        assert_eq!(reader.spec().sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.len(), 1_600);
    }
}
