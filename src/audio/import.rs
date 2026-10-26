//! Décodage de fichiers audio externes (`.wav`, `.mp3`, `.m4a`) et
//! transcodage vers le format de sortie configuré (MP3 ou WAV), pour
//! importer un enregistrement existant dans le dossier de travail — avec la
//! même découpe automatique qu'un enregistrement live (voir
//! [`ChunkedRecorder`]).
//!
//! Ce module sert aussi de brique de décodage à [`super::playback`] : la
//! fonction [`decode_to_mono`] sait lire aussi bien un WAV qu'un MP3 ou un
//! M4A.
//!
//! Le WAV est lu via `hound` (déjà une dépendance du projet). Le MP3 et le
//! M4A/AAC sont décodés via `symphonia`, en 100 % Rust pur : aucune
//! dépendance système (pas de ffmpeg/lame à installer).

use std::path::Path;

use super::dsp::{downmix_f32_to_mono, quantize_to_i16, resample_linear, TARGET_SAMPLE_RATE};
use super::encoder::{ChunkedRecorder, RecordingFormat, RecordingSummary};
use super::error::{AudioError, AudioResult};

/// Extensions de fichiers acceptées par le sélecteur natif lors de l'import
/// (en minuscules, sans le point).
pub const SUPPORTED_IMPORT_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a"];

/// Décode `source_path` (`.wav`, `.mp3` ou `.m4a`), le convertit vers le
/// format canonique mono 16 kHz PCM 16 bits, puis l'encode dans `format`
/// (MP3 ou WAV) au sein de `output_dir`, sous la racine `file_stem`, avec
/// la même découpe que la capture live si `max_bytes` est fourni.
pub fn transcode_to_format(
    source_path: &Path,
    output_dir: &Path,
    file_stem: &str,
    format: RecordingFormat,
    max_bytes: Option<u64>,
) -> AudioResult<RecordingSummary> {
    let (mono, source_rate) = decode_to_mono(source_path)?;
    let resampled = resample_linear(&mono, source_rate, TARGET_SAMPLE_RATE);
    let pcm16 = quantize_to_i16(&resampled);

    let mut recorder = ChunkedRecorder::create(output_dir, file_stem, format, max_bytes)?;
    recorder.write_pcm(&pcm16)?;
    recorder.finalize()
}

/// Décode n'importe quel fichier pris en charge (`.wav`, `.mp3`, `.m4a`)
/// vers un signal mono 16 kHz flottant — exactement le format attendu par
/// le moteur de reconnaissance vocale ([`crate::stt::Transcriber`]).
///
/// Utilisé par l'écriture des notes : comme l'enregistrement peut être un
/// MP3 (format par défaut) ou un WAV, la transcription locale doit pouvoir
/// lire les deux.
pub fn decode_to_mono_16k(source_path: &Path) -> AudioResult<Vec<f32>> {
    let (mono, source_rate) = decode_to_mono(source_path)?;
    Ok(resample_linear(&mono, source_rate, TARGET_SAMPLE_RATE))
}

/// Décode `source_path` vers un signal mono flottant [-1.0, 1.0], à sa
/// fréquence d'échantillonnage native (pas encore ré-échantillonné).
///
/// `pub(crate)` : également utilisé par la lecture audio du dashboard (voir
/// `super::playback`), qui doit pouvoir relire aussi bien un MP3 qu'un WAV.
pub(crate) fn decode_to_mono(source_path: &Path) -> AudioResult<(Vec<f32>, u32)> {
    let extension = source_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "wav" => decode_wav(source_path),
        "mp3" | "m4a" => decode_with_symphonia(source_path),
        other => Err(AudioError::Decode(format!(
            "extension de fichier non prise en charge : « .{other} » (attendu .wav, .mp3 ou .m4a)"
        ))),
    }
}

/// Décode un fichier `.wav` quelconque vers un signal mono flottant. Seuls
/// le PCM 16 bits signé et le flottant 32 bits sont pris en charge : les
/// deux formats WAV les plus répandus. Les autres profondeurs (8/24/32 bits
/// entiers) sont rejetées avec un message clair plutôt que risquer une
/// conversion d'échelle incorrecte.
fn decode_wav(source_path: &Path) -> AudioResult<(Vec<f32>, u32)> {
    let to_read_err = |source: hound::Error| AudioError::WavRead {
        path: source_path.to_path_buf(),
        source,
    };

    let mut reader = hound::WavReader::open(source_path).map_err(to_read_err)?;
    let spec = reader.spec();

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(to_read_err)?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()
            .map_err(to_read_err)?,
        (format, bits) => {
            return Err(AudioError::Decode(format!(
                "format WAV non pris en charge pour l'import ({bits} bits, {format:?}) : \
                 convertissez le fichier en PCM 16 bits ou flottant 32 bits (ex. avec Audacity)"
            )))
        }
    };

    let mono = downmix_f32_to_mono(&interleaved, spec.channels);
    Ok((mono, spec.sample_rate))
}

/// Décode un fichier `.mp3` ou `.m4a` (conteneur ISO/MP4 + codec AAC-LC)
/// via `symphonia`, vers un signal mono flottant.
fn decode_with_symphonia(source_path: &Path) -> AudioResult<(Vec<f32>, u32)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let decode_err = AudioError::Decode;

    let file = std::fs::File::open(source_path).map_err(|e| {
        decode_err(format!(
            "impossible d'ouvrir « {} » : {e}",
            source_path.display()
        ))
    })?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = source_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| decode_err(format!("format audio non reconnu : {e}")))?;
    let mut format = probed.format;

    let (track_id, codec_params) = {
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| decode_err("aucune piste audio décodable trouvée".to_string()))?;
        (track.id, track.codec_params.clone())
    };

    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| decode_err("fréquence d'échantillonnage inconnue".to_string()))?;
    let channels = codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| decode_err(format!("codec audio non pris en charge : {e}")))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(decode_err(format!("erreur de lecture du flux audio : {e}"))),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_buf.is_none() {
                    let spec = *decoded.spec();
                    let duration = decoded.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }
                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(decoded);
                    interleaved.extend_from_slice(buf.samples());
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(decode_err(format!("échec du décodage audio : {e}"))),
        }
    }

    if interleaved.is_empty() {
        return Err(decode_err(
            "aucune donnée audio décodée (fichier vide ou corrompu)".to_string(),
        ));
    }

    let mono = downmix_f32_to_mono(&interleaved, channels);
    Ok((mono, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn rejects_unsupported_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("note.ogg");
        std::fs::write(&path, b"peu importe").unwrap();
        let result = decode_to_mono(&path);
        assert!(matches!(result, Err(AudioError::Decode(_))));
    }

    #[test]
    fn decodes_mono_16_bit_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mono.wav");
        write_test_wav(&path, 16_000, 1, &[0, i16::MAX, i16::MIN + 1, 0]);

        let (mono, rate) = decode_to_mono(&path).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(mono.len(), 4);
        assert!((mono[1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn decodes_and_downmixes_stereo_16_bit_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("stereo.wav");
        // Deux trames stéréo : (max, min) puis (0, 0) → moyennes ~0.0, 0.0.
        write_test_wav(&path, 44_100, 2, &[i16::MAX, i16::MIN + 1, 0, 0]);

        let (mono, rate) = decode_to_mono(&path).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(mono.len(), 2);
        assert!(mono[0].abs() < 1e-3);
        assert_eq!(mono[1], 0.0);
    }

    #[test]
    fn rejects_missing_wav_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("absent.wav");
        let result = decode_to_mono(&path);
        assert!(matches!(result, Err(AudioError::WavRead { .. })));
    }

    #[test]
    fn transcode_produces_a_mono_16k_mp3_with_valid_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.wav");
        // 0,5 s de signal à 48 kHz stéréo.
        let mut samples = Vec::new();
        for i in 0..24_000 {
            let v = if i % 2 == 0 { 6_000 } else { -6_000 };
            samples.push(v);
            samples.push(v);
        }
        write_test_wav(&source, 48_000, 2, &samples);

        let summary = transcode_to_format(
            &source,
            tmp.path(),
            "import_test",
            RecordingFormat::Mp3,
            None,
        )
        .unwrap();

        assert_eq!(summary.paths.len(), 1);
        assert_eq!(summary.paths[0].extension().unwrap(), "mp3");
        let bytes = std::fs::read(&summary.paths[0]).unwrap();
        assert_eq!(bytes[0], 0xFF, "mot de synchronisation MP3 absent");
        assert_eq!(bytes[1] & 0xE0, 0xE0, "mot de synchronisation MP3 invalide");
        assert!(summary.duration.as_secs_f64() > 0.4);
    }

    #[test]
    fn transcode_can_output_wav_too() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.wav");
        write_test_wav(&source, 48_000, 1, &[0; 4_800]);

        let summary = transcode_to_format(
            &source,
            tmp.path(),
            "import_wav",
            RecordingFormat::Wav,
            None,
        )
        .unwrap();

        assert_eq!(summary.paths[0].extension().unwrap(), "wav");
        let reader = hound::WavReader::open(&summary.paths[0]).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, TARGET_SAMPLE_RATE);
        assert!(summary.duration.as_secs_f64() > 0.0);
    }
}
