//! Bibliothèque cœur d'ObsidianRECORDER Desktop : socle audio (capture,
//! encodage MP3, découpe, lecture), raccourci clavier global, reconnaissance
//! vocale hors ligne (Whisper), moteur d'export Obsidian et réglages
//! persistés — entièrement indépendants de l'interface graphique.
//!
//! L'interface (egui/eframe, dans le crate binaire — `main.rs`/`app.rs`)
//! dépend de cette bibliothèque comme n'importe quel autre consommateur,
//! exactement comme les tests d'intégration de `tests/`.

pub mod audio;
pub mod hotkey;
pub mod paths;
pub mod settings;
pub mod stt;
pub mod vault;
