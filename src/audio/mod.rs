//! Socle audio d'ObsidianRECORDER Desktop : capture bas niveau (WASAPI via
//! `cpal`), pipeline DSP pur (downmix + ré-échantillonnage 16 kHz) et
//! écriture WAV conforme aux moteurs STT hors ligne.

pub mod capture;
pub mod dsp;
pub mod error;
pub mod playback;
pub mod wav_writer;

pub use capture::{list_input_devices, AudioCapture, RecordingSummary};
pub use error::{AudioError, AudioResult};
pub use playback::AudioPlayback;
