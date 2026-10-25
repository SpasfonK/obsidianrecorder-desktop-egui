//! Raccourcis clavier globaux : détection de collision pure et testable
//! ([`registry`]), représentation normalisée d'un raccourci ([`binding`]),
//! et couche d'enregistrement réel auprès de Windows ([`os_backend`]).

pub mod binding;
pub mod error;
pub mod os_backend;
pub mod registry;

pub use binding::HotkeyBinding;
pub use error::{HotkeyError, HotkeyResult};
pub use os_backend::OsHotkeyBackend;
pub use registry::{HotkeyAction, HotkeyId, HotkeyRegistry};
