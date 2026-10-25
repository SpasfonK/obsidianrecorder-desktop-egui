//! Réglages persistés de l'application : chemin du Vault choisi, mode
//! d'écriture, tags par défaut, préférence de thème.
//!
//! Sérialisés en JSON dans `%LOCALAPPDATA%\ObsidianRECORDER\settings.json`
//! — un simple fichier, pas de base de données, `serde`/`serde_json` étant
//! déjà des dépendances de compilation extrêmement répandues et compilées
//! statiquement (aucun impact sur le principe « zéro dépendance » au sens
//! runtime visé par le projet).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Mode d'écriture des notes, persisté (reflète `vault::WriteMode` au
/// niveau des réglages, sans dépendre d'un chemin concret).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteModeSetting {
    #[default]
    NewFile,
    DailyNote,
}

/// Préférence de thème visuel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

/// Réglages persistés de l'application.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Settings {
    pub vault_path: Option<PathBuf>,
    #[serde(default)]
    pub write_mode: WriteModeSetting,
    #[serde(default)]
    pub extra_tags: Vec<String>,
    #[serde(default)]
    pub theme: ThemePreference,
}

/// Chemin du fichier de réglages, dans le profil utilisateur courant
/// (aucun droit administrateur requis).
pub fn settings_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("ObsidianRECORDER").join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

/// Charge les réglages depuis leur emplacement standard. Tout problème
/// (fichier absent, JSON invalide) retombe silencieusement sur les valeurs
/// par défaut plutôt que d'empêcher l'application de démarrer.
pub fn load() -> Settings {
    load_from(&settings_path())
}

/// Enregistre les réglages à leur emplacement standard.
pub fn save(settings: &Settings) -> std::io::Result<()> {
    save_to(&settings_path(), settings)
}

fn load_from(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            vault_path: Some(PathBuf::from("C:/MonVault")),
            write_mode: WriteModeSetting::DailyNote,
            extra_tags: vec!["perso".to_string(), "projet-x".to_string()],
            theme: ThemePreference::Light,
        }
    }

    #[test]
    fn settings_round_trip_via_json_string() {
        let json = serde_json::to_string(&sample()).unwrap();
        let parsed: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, sample());
    }

    #[test]
    fn missing_fields_fall_back_to_serde_defaults() {
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn default_write_mode_is_new_file() {
        assert_eq!(Settings::default().write_mode, WriteModeSetting::NewFile);
    }

    #[test]
    fn default_theme_is_system() {
        assert_eq!(Settings::default().theme, ThemePreference::System);
    }

    #[test]
    fn save_then_load_round_trip_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        save_to(&path, &sample()).unwrap();
        assert_eq!(load_from(&path), sample());
    }

    #[test]
    fn load_from_missing_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.json");
        assert_eq!(load_from(&path), Settings::default());
    }

    #[test]
    fn load_from_corrupted_file_returns_defaults_instead_of_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("corrupted.json");
        std::fs::write(&path, "ceci n'est pas du JSON valide {{{").unwrap();
        assert_eq!(load_from(&path), Settings::default());
    }
}
