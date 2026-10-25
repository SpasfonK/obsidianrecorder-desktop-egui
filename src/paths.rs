//! Emplacements par défaut utilisés tant que la sélection du Vault
//! Obsidian (Itération 3) n'est pas encore implémentée dans l'interface.

use std::path::{Path, PathBuf};

/// Dossier de l'exécutable courant : base de la distribution **portable**
/// de l'application. Le modèle Whisper (voir `stt::model`) y est cherché
/// directement, à côté du `.exe`, pour que le dossier reste auto-suffisant
/// et déplaçable tel quel, sans rien écrire dans le profil utilisateur.
pub fn portable_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Dossier de secours où sont déposés les enregistrements :
/// `Documents\ObsidianRECORDER` dans le profil de l'utilisateur Windows
/// courant. Sera remplacé par le sous-dossier choisi dans le Vault Obsidian
/// de l'utilisateur en Itération 3.
pub fn fallback_recordings_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Documents").join("ObsidianRECORDER"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Construit un nom de fichier `.wav` horodaté (ex. `rec_20260101_143000.wav`),
/// en écho au nommage `attachments/rec_YYYYMMDD_HHmmss.wav` prévu par le
/// frontmatter Markdown des itérations suivantes.
pub fn timestamped_wav_filename() -> String {
    format!("rec_{}.wav", chrono::Local::now().format("%Y%m%d_%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_dir_is_an_existing_directory() {
        let dir = portable_dir();
        assert!(
            dir.is_dir(),
            "portable_dir() doit désigner un dossier existant : {}",
            dir.display()
        );
    }

    #[test]
    fn portable_dir_is_the_current_executable_parent() {
        let exe = std::env::current_exe().expect("current_exe() doit être disponible en test");
        let parent = exe.parent().expect("l'exécutable doit avoir un dossier parent");
        assert_eq!(portable_dir(), parent.to_path_buf());
    }
}
