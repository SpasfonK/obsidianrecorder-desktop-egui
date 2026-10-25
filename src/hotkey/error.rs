//! Erreurs du sous-système de raccourcis clavier globaux.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("le raccourci « {0} » n'a pas pu être analysé : accélérateur invalide")]
    InvalidAccelerator(String),

    #[error("le raccourci « {0} » est déjà enregistré pour une autre action")]
    AlreadyBound(String),

    #[error("aucun raccourci n'est enregistré sous l'identifiant {0}")]
    UnknownBinding(u32),

    #[error("échec de l'enregistrement du raccourci auprès du système d'exploitation : {0}")]
    OsRegistration(String),

    #[error("échec de la désinscription du raccourci auprès du système d'exploitation : {0}")]
    OsUnregistration(String),
}

pub type HotkeyResult<T> = Result<T, HotkeyError>;
