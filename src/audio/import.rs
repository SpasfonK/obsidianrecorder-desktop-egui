//! Import de fichiers audio externes (`.wav`, `.mp3`, `.m4a`) pour une
//! transcription en différé : décodage vers un signal mono flottant,
//! conversion vers le format canonique de l'application (mono 16 kHz, via
//! le même pipeline DSP que la capture live — voir `super::dsp`), puis
//! écriture d'un fichier `.wav` standard directement réutilisable par
//! `spawn_transcription` et par l'écriture dans le Vault, exactement
//! comme un enregistrement live.
//!
//! Le WAV est lu via `hound` (déjà une dépendance du projet). Le MP3 et le
//! M4A/AAC sont décodés via `symphonia`, en 100 % Rust pur : aucune
//! dépendance système (pas de ffmpeg/lame à installer), cohérent avec le
//! reste du projet.

use std::path::{Path, PathBuf};

use super::dsp::{downmix_f32_to_mono, quantize_to_i16, resample_linear, TARGET_SAMPLE_RATE};
use super::error::{AudioError, AudioResult};
use super::wav_writer::RecordingWavWriter;

/// Extensions de fichiers acceptées par le sélecteur natif lors de
/// l'import (en minuscules, sans le point).
pub const SUPPORTED_IMPORT_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a"];

/// Décode `source_path` (`.wav`, `.mp3` ou `.m4a`), le convertit vers le
/// format canonique mono 16 kHz PCM 16 bits et l'écrit dans
/// `destination_path`. Retourne la durée du fichier produit, pour un
/// affichage immédiat avant la fin de la transcription.
pub fn decode_to_canonical_wav(
    source_path: &Path,
    destination_path: &Path,
) -> AudioResult<std::time::Duration> {
    let (mono, source_rate) = decode_to_mono(source_path)?;
    let resampled = resample_linear(&mono, source_rate, TARGET_SAMPLE_RATE);
    let pcm16 = quantize_to_i16(&resampled);

    let mut writer = RecordingWavWriter::create(destination_path)?;
    writer.write_samples(&pcm16)?;
    let duration = writer.duration();
    writer.finalize()?;
    Ok(duration)
}

/// Décode `source_path` vers un signal mono flottant [-1.0, 1.0], à sa
/// fréquence d'échantillonnage native (pas encore ré-échantillonné).
fn decode_to_mono(source_path: &Path) -> AudioResult<(Vec<f32>, u32)> {
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

/// Décode un fichier `.wav` quelconque (pas nécessairement produit par
/// cette application) vers un signal mono flottant. Seuls le PCM 16 bits
/// signé et le flottant 32 bits sont pris en charge : ce sont de loin les
/// deux formats WAV les plus répandus, et les deux déjà manipulés ailleurs
/// dans ce projet (voir `stt::transcriber` et `audio::playback`). Les
/// autres profondeurs (8/24/32 bits entiers) sont rejetées avec un message
/// clair plutôt que risquer une conversion d'échelle incorrecte.
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
    fn decode_to_canonical_wav_produces_a_readable_mono_16k_file() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source.wav");
        write_test_wav(&source, 48_000, 1, &[0; 4_800]); // 0,1 s de silence à 48 kHz

        let destination = tmp.path().join("converti.wav");
        let duration = decode_to_canonical_wav(&source, &destination).unwrap();

        assert!(destination.is_file());
        let reader = hound::WavReader::open(&destination).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
        assert!(duration.as_secs_f64() > 0.0);
    }
}
