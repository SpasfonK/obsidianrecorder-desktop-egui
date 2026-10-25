//! Reconnaissance vocale locale et hors ligne, via Whisper
//! (`whisper-rs`/whisper.cpp, lié statiquement — aucun `.dll` séparé à
//! distribuer). Aucun appel réseau lors de la transcription elle-même :
//! seul le téléchargement initial du modèle (voir [`model`] et
//! `scripts/download-model.ps1`) nécessite une connexion, une seule fois.

pub mod error;
pub mod model;
pub mod transcriber;

pub use error::{SttError, SttResult};
pub use transcriber::{Transcript, TranscriptSegment, Transcriber};
