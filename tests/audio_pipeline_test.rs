//! Test d'intégration correspondant au critère du jalon 1 : « Test de
//! capture d'un flux audio simulé et écriture d'un fichier WAV valide ».
//!
//! On simule ici exactement ce que produirait le callback `cpal` (un
//! buffer entrelacé multi-canaux), puis on fait passer ce buffer par le
//! pipeline réel utilisé en production (`audio::capture`) : downmix mono,
//! ré-échantillonnage 16 kHz, quantification PCM 16 bits, écriture WAV. Le
//! matériel audio et le driver WASAPI eux-mêmes ne peuvent pas être
//! simulés de façon déterministe ; c'est pourquoi ce test couvre tout le
//! pipeline de traitement, qui est la partie testable et la plus sujette à
//! régression (erreurs d'arrondi, de canaux, de format WAV).

use std::f32::consts::PI;

use obsidianrecorder_lib::audio::dsp::{
    downmix_f32_to_mono, quantize_to_i16, resample_linear, TARGET_SAMPLE_RATE,
};
use obsidianrecorder_lib::audio::wav_writer::RecordingWavWriter;

/// Génère un buffer stéréo entrelacé simulant un signal capturé par la
/// carte son (une tonalité à 440 Hz), comme le ferait `cpal` dans son
/// callback d'entrée.
fn simulate_captured_buffer(sample_rate: u32, seconds: f32, channels: u16) -> Vec<f32> {
    let frame_count = (sample_rate as f32 * seconds) as usize;
    let mut buffer = Vec::with_capacity(frame_count * channels as usize);
    for i in 0..frame_count {
        let t = i as f32 / sample_rate as f32;
        let sample = (2.0 * PI * 440.0 * t).sin() * 0.8;
        for _ in 0..channels {
            buffer.push(sample);
        }
    }
    buffer
}

#[test]
fn simulated_stereo_capture_produces_valid_16khz_mono_wav() {
    const SOURCE_RATE: u32 = 44_100;
    const CHANNELS: u16 = 2;
    const DURATION_SECONDS: f32 = 0.5;

    let captured = simulate_captured_buffer(SOURCE_RATE, DURATION_SECONDS, CHANNELS);

    // Pipeline identique à celui utilisé par `audio::capture` en production.
    let mono = downmix_f32_to_mono(&captured, CHANNELS);
    let resampled = resample_linear(&mono, SOURCE_RATE, TARGET_SAMPLE_RATE);
    let pcm16 = quantize_to_i16(&resampled);

    let tmp_dir = tempfile::tempdir().expect("création du dossier temporaire");
    let wav_path = tmp_dir.path().join("simulated_recording.wav");

    let mut writer = RecordingWavWriter::create(&wav_path).expect("création du writer WAV");
    writer.write_samples(&pcm16).expect("écriture des échantillons");
    let written_samples = writer.samples_written();
    let finalized_path = writer.finalize().expect("finalisation du fichier WAV");

    assert_eq!(finalized_path, wav_path);
    assert_eq!(written_samples, pcm16.len() as u64);

    // Le fichier doit être un WAV valide, mono, 16 kHz, PCM 16 bits.
    let mut reader = hound::WavReader::open(&wav_path).expect("le fichier WAV doit être lisible");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
    assert_eq!(spec.bits_per_sample, 16);
    assert_eq!(spec.sample_format, hound::SampleFormat::Int);

    let decoded: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .expect("les échantillons doivent être décodables");
    assert_eq!(decoded.len(), pcm16.len());

    // ~0,5 s à 16 kHz : on tolère une marge de quelques échantillons due au
    // ré-échantillonnage linéaire.
    let expected_len = (TARGET_SAMPLE_RATE as f32 * DURATION_SECONDS) as usize;
    assert!((decoded.len() as i64 - expected_len as i64).abs() <= 4);
}

#[test]
fn mono_capture_at_target_rate_is_written_without_resampling_artifacts() {
    // Cas où le périphérique expose déjà nativement du mono 16 kHz : le
    // ré-échantillonnage doit être un no-op et préserver les échantillons.
    let captured = simulate_captured_buffer(TARGET_SAMPLE_RATE, 0.2, 1);
    let mono = downmix_f32_to_mono(&captured, 1);
    let resampled = resample_linear(&mono, TARGET_SAMPLE_RATE, TARGET_SAMPLE_RATE);
    assert_eq!(mono, resampled);

    let pcm16 = quantize_to_i16(&resampled);
    let tmp_dir = tempfile::tempdir().expect("création du dossier temporaire");
    let wav_path = tmp_dir.path().join("mono_recording.wav");

    let mut writer = RecordingWavWriter::create(&wav_path).expect("création du writer WAV");
    writer.write_samples(&pcm16).expect("écriture des échantillons");
    writer.finalize().expect("finalisation du fichier WAV");

    let reader = hound::WavReader::open(&wav_path).expect("le fichier WAV doit être lisible");
    assert_eq!(reader.duration() as usize, pcm16.len());
}

#[test]
fn empty_recording_still_produces_a_structurally_valid_wav_file() {
    // Un enregistrement démarré puis arrêté immédiatement (0 échantillon)
    // ne doit pas produire de fichier corrompu.
    let tmp_dir = tempfile::tempdir().expect("création du dossier temporaire");
    let wav_path = tmp_dir.path().join("empty_recording.wav");

    let writer = RecordingWavWriter::create(&wav_path).expect("création du writer WAV");
    writer.finalize().expect("finalisation du fichier WAV vide");

    let reader = hound::WavReader::open(&wav_path).expect("le fichier WAV doit rester lisible");
    assert_eq!(reader.duration(), 0);
}
