//! Emplacement et présence du modèle Whisper (GGML).
//!
//! Le fichier de modèle n'est jamais embarqué dans le binaire ni dans le
//! dépôt : il pèse de plusieurs centaines de Mo à plus d'1,5 Go, bien au-delà
//! de ce qu'il est raisonnable de committer. Voir `scripts/download-model.ps1`
//! et le README pour l'obtenir — c'est la seule étape de tout le projet qui
//! nécessite une connexion réseau, et une seule fois.

use std::path::PathBuf;

/// Nom de fichier du modèle recommandé par défaut : Whisper large-v3-turbo,
/// quantifié en q5_0 (~574 Mo) — le meilleur compromis qualité/taille pour
/// un usage hors ligne local sur une machine grand public. La variante
/// pleine précision (`ggml-large-v3-turbo.bin`, ~1,62 Go) reste utilisable
/// en remplaçant simplement ce fichier, sans changement de code.
pub const DEFAULT_MODEL_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";

/// URL officielle du modèle recommandé (dépôt Hugging Face de whisper.cpp).
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";

/// Dossier où sont rangés les modèles Whisper, dans le profil utilisateur
/// courant : aucun droit administrateur requis, cohérent avec le reste de
/// l'application (voir `paths::fallback_recordings_dir`).
pub fn models_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|dir| dir.join("ObsidianRECORDER").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

/// Chemin complet vers le modèle par défaut.
pub fn default_model_path() -> PathBuf {
    models_dir().join(DEFAULT_MODEL_FILENAME)
}

/// Indique si le modèle par défaut est déjà présent sur le disque.
pub fn default_model_is_present() -> bool {
    default_model_path().is_file()
}
