//! Erreurs du moteur Obsidian (validation du Vault, écriture des notes).

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("« {0} » n'existe pas ou n'est pas un dossier")]
    NotADirectory(PathBuf),

    #[error("« {0} » ne semble pas être un Vault Obsidian (dossier .obsidian introuvable)")]
    NotAnObsidianVault(PathBuf),

    #[error("erreur d'entrée/sortie : {0}")]
    Io(#[from] std::io::Error),
}

pub type VaultResult<T> = Result<T, VaultError>;
