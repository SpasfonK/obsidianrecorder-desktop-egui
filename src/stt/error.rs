//! Erreurs de la reconnaissance vocale hors ligne (Whisper).

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SttError {
    #[error(
        "modèle Whisper introuvable : {0}\n\
         Téléchargez-le avec scripts/download-model.ps1, ou placez-le manuellement à cet emplacement."
    )]
    ModelNotFound(PathBuf),

    #[error("impossible de charger le modèle Whisper « {path} » : {message}")]
    ModelLoad { path: PathBuf, message: String },

    #[error("impossible de créer l'état de transcription Whisper : {0}")]
    StateCreation(String),

    #[error("échec de la transcription : {0}")]
    Inference(String),

    #[error("erreur de lecture du fichier audio : {0}")]
    AudioRead(String),
}

pub type SttResult<T> = Result<T, SttError>;
