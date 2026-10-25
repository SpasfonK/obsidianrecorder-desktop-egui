//! Moteur Obsidian : validation du Vault, génération du frontmatter/note
//! Markdown, et écriture sécurisée sur le disque (atomique, avec gestion
//! des conflits de nommage et des modes nouvelle-note / daily-note).

pub mod dashboard;
pub mod error;
pub mod frontmatter;
pub mod note;
pub mod writer;

use std::path::{Path, PathBuf};

pub use error::{VaultError, VaultResult};
pub use frontmatter::FrontmatterData;
pub use note::NoteData;
pub use writer::{write_note, WriteMode};

/// Représente un Vault Obsidian déjà validé (le dossier `.obsidian` a été
/// vérifié présent à l'ouverture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Ouvre et valide un Vault Obsidian à la racine `root` : `root` doit
    /// être un dossier existant contenant un sous-dossier `.obsidian` (créé
    /// par Obsidian lui-même à la première ouverture du Vault).
    pub fn open(root: impl Into<PathBuf>) -> VaultResult<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(VaultError::NotADirectory(root));
        }
        if !root.join(".obsidian").is_dir() {
            return Err(VaultError::NotAnObsidianVault(root));
        }
        Ok(Self { root })
    }

    /// Chemin racine du Vault.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Résout un chemin relatif au Vault (ex. sous-dossier "AudioNotes").
    pub fn resolve(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_directory() {
        let result = Vault::open("/chemin/qui/n_existe/vraiment/pas/obsidianrecorder");
        assert!(matches!(result, Err(VaultError::NotADirectory(_))));
    }

    #[test]
    fn rejects_directory_without_dot_obsidian() {
        let tmp = tempfile::tempdir().unwrap();
        let result = Vault::open(tmp.path());
        assert!(matches!(result, Err(VaultError::NotAnObsidianVault(_))));
    }

    #[test]
    fn accepts_directory_with_dot_obsidian() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".obsidian")).unwrap();
        let vault = Vault::open(tmp.path()).expect("doit être accepté");
        assert_eq!(vault.root(), tmp.path());
    }

    #[test]
    fn resolve_joins_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".obsidian")).unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        assert_eq!(vault.resolve("AudioNotes"), tmp.path().join("AudioNotes"));
    }
}
