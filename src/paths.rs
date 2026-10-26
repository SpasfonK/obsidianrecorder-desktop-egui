//! Emplacements par défaut et nommage des fichiers audio.

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
/// courant. Utilisé pour les enregistrements bruts et pour les fichiers
/// produits par l'import/transcodage, en plus du Vault Obsidian si celui-ci
/// est configuré.
pub fn fallback_recordings_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Documents").join("ObsidianRECORDER"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Racine horodatée d'un enregistrement (ex. `rec_20260101_143000`), sans
/// extension : la même racine est utilisée pour tous les morceaux d'un
/// enregistrement découpé (`rec_..._part1.mp3`, `rec_..._part2.mp3`, …),
/// pour que les fichiers d'une même session restent faciles à rapprocher.
pub fn timestamped_stem() -> String {
    format!("rec_{}", chrono::Local::now().format("%Y%m%d_%H%M%S"))
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

    #[test]
    fn timestamped_stem_has_the_expected_shape() {
        let stem = timestamped_stem();
        assert!(stem.starts_with("rec_"), "préfixe inattendu : {stem}");
        // `rec_` + `YYYYMMDD_HHMMSS` = 4 + 15 caractères.
        assert_eq!(stem.len(), 19, "longueur inattendue : {stem}");
        let digits = stem.trim_start_matches("rec_");
        let parts: Vec<&str> = digits.split('_').collect();
        assert_eq!(parts.len(), 2, "séparateur `_` manquant : {stem}");
        assert_eq!(parts[0].len(), 8, "date inattendue : {stem}");
        assert_eq!(parts[1].len(), 6, "heure inattendue : {stem}");
        assert!(
            digits.chars().all(|c| c.is_ascii_digit() || c == '_'),
            "caractères inattendus : {stem}"
        );
    }
}
