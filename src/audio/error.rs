//! Erreurs du socle audio (capture, ré-échantillonnage, écriture WAV).

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("aucun périphérique d'entrée audio n'a été trouvé sur ce système")]
    NoInputDevice,

    #[error("le périphérique d'entrée « {0} » est introuvable ou a été déconnecté")]
    DeviceNotFound(String),

    #[error("impossible d'interroger la configuration du périphérique : {0}")]
    ConfigQuery(String),

    #[error("impossible de construire le flux audio d'entrée : {0}")]
    BuildStream(String),

    #[error("impossible de démarrer le flux audio : {0}")]
    PlayStream(String),

    #[error("un enregistrement est déjà en cours")]
    AlreadyRecording,

    #[error("aucun enregistrement n'est en cours")]
    NotRecording,

    #[error("erreur d'écriture du fichier WAV « {path} » : {source}")]
    WavWrite {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },

    #[error("erreur de lecture du fichier WAV « {path} » : {source}")]
    WavRead {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },

    #[error("aucun périphérique de sortie audio n'a été trouvé sur ce système")]
    NoOutputDevice,

    #[error("une lecture audio est déjà en cours")]
    AlreadyPlaying,

    #[error("aucune lecture audio n'est en cours")]
    NotPlaying,

    #[error("impossible de décoder le fichier audio importé : {0}")]
    Decode(String),

    #[error("erreur d'accès au fichier « {path} » : {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("impossible d'initialiser l'encodeur MP3 : {0}")]
    Mp3Init(String),

    #[error("erreur d'encodage MP3 : {0}")]
    Mp3Encode(String),
}

pub type AudioResult<T> = Result<T, AudioError>;
