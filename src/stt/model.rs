//! Emplacement et présence du modèle Whisper (GGML).
//!
//! Le fichier de modèle n'est jamais embarqué dans le binaire ni dans le
//! dépôt : il pèse de plusieurs centaines de Mo à plus d'1,5 Go, bien au-delà
//! de ce qu'il est raisonnable de committer. Voir `scripts/download-model.ps1`
//! et le README pour l'obtenir — c'est la seule étape de tout le projet qui
//! nécessite une connexion réseau, et une seule fois.

use std::path::PathBuf;

use crate::paths;

/// Nom de fichier du modèle recommandé par défaut : Whisper large-v3-turbo,
/// quantifié en q5_0 (~574 Mo) — le meilleur compromis qualité/taille pour
/// un usage hors ligne local sur une machine grand public. La variante
/// pleine précision (`ggml-large-v3-turbo.bin`, ~1,62 Go) reste utilisable
/// en remplaçant simplement ce fichier, sans changement de code.
pub const DEFAULT_MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";

/// URL officielle du modèle recommandé (dépôt Hugging Face de whisper.cpp).
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";

/// Dossier où sont rangés les modèles Whisper : le dossier de l'exécutable
/// lui-même (voir `paths::portable_dir`), afin que l'application soit
/// réellement **portable** — le fichier `ggml-*.bin` se déplace avec le
/// `.exe`, sans rien écrire dans le profil utilisateur ni exiger de droits
/// particuliers.
pub fn models_dir() -> PathBuf {
    paths::portable_dir()
}

/// Ancien emplacement des modèles (`%LOCALAPPDATA%\ObsidianRECORDER\models`),
/// conservé uniquement comme **repli** de compatibilité : il évite de forcer
/// un re-téléchargement de plusieurs centaines de Mo aux installations
/// existantes, sans changer la cible de téléchargement (dossier portable).
fn legacy_models_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("ObsidianRECORDER").join("models"))
}

/// Chemin complet vers le modèle par défaut : à côté de l'exécutable en
/// priorité, sinon l'ancien emplacement `%LOCALAPPDATA%` s'il contient déjà
/// un modèle (compatibilité ascendante), sinon l'emplacement portable — la
/// cible du téléchargement (`scripts/download-model.ps1`).
pub fn default_model_path() -> PathBuf {
    let portable = models_dir().join(DEFAULT_MODEL_FILENAME);
    if portable.is_file() {
        return portable;
    }
    if let Some(legacy) = legacy_models_dir() {
        let legacy = legacy.join(DEFAULT_MODEL_FILENAME);
        if legacy.is_file() {
            return legacy;
        }
    }
    portable
}

/// Indique si le modèle par défaut est déjà présent sur le disque, à
/// l'emplacement portable ou à l'ancien emplacement de repli.
pub fn default_model_is_present() -> bool {
    default_model_path().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_dir_lives_next_to_the_executable() {
        assert_eq!(models_dir(), crate::paths::portable_dir());
    }

    #[test]
    fn default_model_path_targets_the_model_filename() {
        assert_eq!(
            default_model_path().file_name().and_then(|name| name.to_str()),
            Some(DEFAULT_MODEL_FILENAME)
        );
    }
}
