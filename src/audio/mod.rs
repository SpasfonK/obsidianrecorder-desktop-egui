//! Socle audio d'ObsidianRECORDER Desktop : capture bas niveau (WASAPI via
//! `cpal`), pipeline DSP pur (downmix + ré-échantillonnage 16 kHz),
//! encodage MP3 réel (LAME) ou WAV, découpe automatique en plusieurs
//! fichiers, décodage/import de fichiers externes et lecture de
//! prévisualisation.

pub mod capture;
pub mod dsp;
pub mod encoder;
pub mod error;
pub mod import;
pub mod playback;
pub mod wav_writer;

pub use capture::{list_input_devices, AudioCapture, CaptureConfig};
pub use encoder::{chunk_bytes_from_mb, ChunkedRecorder, RecordingFormat, RecordingSummary};
pub use error::{AudioError, AudioResult};
pub use import::{decode_to_mono_16k, transcode_to_format, SUPPORTED_IMPORT_EXTENSIONS};
pub use playback::AudioPlayback;
