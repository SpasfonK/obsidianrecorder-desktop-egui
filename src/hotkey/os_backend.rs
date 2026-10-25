//! Implémentation réelle de l'enregistrement de raccourcis auprès du
//! système d'exploitation, via la crate `global-hotkey` (`RegisterHotKey`
//! sous Windows).
//!
//! Cette couche n'est volontairement pas couverte par des tests unitaires :
//! `global-hotkey` exige un event loop Win32 actif sur le thread courant
//! (voir sa documentation), ce qui n'a de sens que dans l'application
//! elle-même — le runtime `eframe`/`winit` fait déjà tourner cette boucle
//! sur le thread principal. Toute la logique métier testable (détection de
//! collision, cycle de vie des identifiants) vit dans
//! [`super::registry::HotkeyRegistry`], qui ne dépend d'aucune API système.

use std::collections::HashMap;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::GlobalHotKeyManager;

use super::binding::HotkeyBinding;
use super::error::{HotkeyError, HotkeyResult};

/// Pont entre les [`HotkeyBinding`] normalisés de ce projet et le
/// `GlobalHotKeyManager` natif de `global-hotkey`.
pub struct OsHotkeyBackend {
    manager: GlobalHotKeyManager,
    /// Associe l'identifiant natif `global-hotkey` (calculé à partir des
    /// modificateurs + de la touche) au `HotKey` correspondant, nécessaire
    /// pour appeler `unregister`.
    native: HashMap<u32, HotKey>,
}

impl OsHotkeyBackend {
    /// Crée le gestionnaire natif. Doit être appelé sur le thread qui fait
    /// tourner la boucle d'événements de l'application (le thread principal
    /// d'`eframe` convient).
    pub fn new() -> HotkeyResult<Self> {
        let manager =
            GlobalHotKeyManager::new().map_err(|e| HotkeyError::OsRegistration(e.to_string()))?;
        Ok(Self {
            manager,
            native: HashMap::new(),
        })
    }

    /// Enregistre réellement le raccourci auprès de Windows et retourne
    /// l'identifiant natif `global-hotkey` (utile pour faire correspondre
    /// les événements reçus au raccourci enregistré).
    pub fn register(&mut self, binding: &HotkeyBinding) -> HotkeyResult<u32> {
        let modifiers = modifiers_bitflags(binding);
        let code = code_from_key(binding.key())?;
        let hotkey = HotKey::new(if modifiers.is_empty() { None } else { Some(modifiers) }, code);

        self.manager
            .register(hotkey)
            .map_err(|e| HotkeyError::OsRegistration(e.to_string()))?;

        let native_id = hotkey.id();
        self.native.insert(native_id, hotkey);
        Ok(native_id)
    }

    /// Désinscrit un raccourci précédemment enregistré via [`Self::register`].
    pub fn unregister(&mut self, native_id: u32) -> HotkeyResult<()> {
        let hotkey = self
            .native
            .remove(&native_id)
            .ok_or(HotkeyError::UnknownBinding(native_id))?;
        self.manager
            .unregister(hotkey)
            .map_err(|e| HotkeyError::OsUnregistration(e.to_string()))?;
        Ok(())
    }
}

/// Traduit les modificateurs normalisés du projet vers les bitflags
/// `Modifiers` de `global-hotkey`.
fn modifiers_bitflags(binding: &HotkeyBinding) -> Modifiers {
    let mut mods = Modifiers::empty();
    for m in binding.modifiers() {
        mods |= match *m {
            "CTRL" => Modifiers::CONTROL,
            "ALT" => Modifiers::ALT,
            "SHIFT" => Modifiers::SHIFT,
            "META" => Modifiers::META,
            _ => Modifiers::empty(),
        };
    }
    mods
}

/// Traduit une touche principale normalisée (ex. "O", "F5", "SPACE") vers
/// le `Code` physique attendu par `global-hotkey`. Couvre les lettres,
/// chiffres, touches de fonction F1-F12 et les touches de navigation/édition
/// les plus courantes — largement suffisant pour les raccourcis de bascule
/// d'enregistrement visés par le cahier des charges.
fn code_from_key(key: &str) -> HotkeyResult<Code> {
    let upper = key.to_ascii_uppercase();
    let code = match upper.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "TAB" => Code::Tab,
        "ESC" | "ESCAPE" => Code::Escape,
        "BACKSPACE" => Code::Backspace,
        "DELETE" | "DEL" => Code::Delete,
        "UP" | "ARROWUP" => Code::ArrowUp,
        "DOWN" | "ARROWDOWN" => Code::ArrowDown,
        "LEFT" | "ARROWLEFT" => Code::ArrowLeft,
        "RIGHT" | "ARROWRIGHT" => Code::ArrowRight,
        "HOME" => Code::Home,
        "END" => Code::End,
        "PAGEUP" => Code::PageUp,
        "PAGEDOWN" => Code::PageDown,
        "INSERT" => Code::Insert,
        _ => {
            return Err(HotkeyError::InvalidAccelerator(format!(
                "touche non reconnue : « {key} »"
            )))
        }
    };
    Ok(code)
}
