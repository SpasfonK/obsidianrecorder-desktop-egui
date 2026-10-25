//! Emplacements par défaut utilisés tant que la sélection du Vault
//! Obsidian (Itération 3) n'est pas encore implémentée dans l'interface.

use std::path::PathBuf;

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
