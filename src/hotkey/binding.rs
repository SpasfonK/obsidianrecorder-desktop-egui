//! Représentation normalisée d'un raccourci clavier global, indépendante de
//! la bibliothèque système utilisée pour l'enregistrement réel
//! (`global-hotkey`). Cette normalisation est ce qui permet une détection
//! de collision fiable dans [`super::registry::HotkeyRegistry`], quelle que
//! soit la façon dont l'utilisateur (ou le code) a écrit l'accélérateur.

use std::fmt;

use super::error::HotkeyError;

/// Ordre canonique d'affichage/comparaison des modificateurs.
const MODIFIER_ORDER: [&str; 4] = ["CTRL", "ALT", "SHIFT", "META"];

/// Un raccourci clavier global normalisé (ex. « Ctrl+Shift+O »).
///
/// La normalisation (ordre des modificateurs, casse) garantit que deux
/// écritures différentes du même raccourci (« ctrl+shift+o » et
/// « Shift+Ctrl+O ») sont reconnues comme identiques par le détecteur de
/// collisions du registre.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotkeyBinding {
    /// Modificateurs normalisés, triés et dédupliqués (parmi "CTRL", "ALT",
    /// "SHIFT", "META" — "META" désigne la touche Windows/Cmd).
    modifiers: Vec<&'static str>,
    /// Touche principale normalisée en majuscules (ex. "O", "F5", "SPACE").
    key: String,
}

impl HotkeyBinding {
    /// Analyse un accélérateur textuel tel que « Ctrl+Shift+O » ou
    /// « Win+Alt+R ». Les alias `Win`/`Cmd`/`Command`/`Meta` sont acceptés
    /// comme synonymes de la touche Windows/Cmd (« META »).
    pub fn parse(accelerator: &str) -> Result<Self, HotkeyError> {
        let trimmed = accelerator.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::InvalidAccelerator(accelerator.to_string()));
        }

        let mut modifiers: Vec<&'static str> = Vec::new();
        let mut key: Option<String> = None;

        for part in trimmed.split('+') {
            let part = part.trim();
            if part.is_empty() {
                // Cas "Ctrl++O" ou accélérateur commençant/finissant par "+".
                return Err(HotkeyError::InvalidAccelerator(accelerator.to_string()));
            }
            match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" | "CTL" => modifiers.push("CTRL"),
                "ALT" | "OPTION" => modifiers.push("ALT"),
                "SHIFT" => modifiers.push("SHIFT"),
                "WIN" | "SUPER" | "CMD" | "COMMAND" | "META" => modifiers.push("META"),
                upper => {
                    if key.is_some() {
                        // Deux touches non-modificatrices : accélérateur invalide.
                        return Err(HotkeyError::InvalidAccelerator(accelerator.to_string()));
                    }
                    key = Some(upper.to_string());
                }
            }
        }

        let key = key.ok_or_else(|| HotkeyError::InvalidAccelerator(accelerator.to_string()))?;

        modifiers.sort_by_key(|m| MODIFIER_ORDER.iter().position(|o| o == m).unwrap_or(99));
        modifiers.dedup();

        Ok(Self { modifiers, key })
    }

    /// Représentation canonique utilisée pour la comparaison/collision
    /// (ex. `"CTRL+SHIFT+O"`).
    pub fn canonical(&self) -> String {
        if self.modifiers.is_empty() {
            self.key.clone()
        } else {
            format!("{}+{}", self.modifiers.join("+"), self.key)
        }
    }

    /// Modificateurs normalisés (utilisé par la couche d'enregistrement
    /// système pour construire les `Modifiers` natifs).
    pub fn modifiers(&self) -> &[&'static str] {
        &self.modifiers
    }

    /// Touche principale normalisée (utilisée par la couche
    /// d'enregistrement système pour résoudre le `Code` natif).
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for modifier in &self.modifiers {
            write!(f, "{}+", display_case(modifier))?;
        }
        write!(f, "{}", self.key)
    }
}

fn display_case(modifier: &str) -> &'static str {
    match modifier {
        "CTRL" => "Ctrl",
        "ALT" => "Alt",
        "SHIFT" => "Shift",
        "META" => "Win",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_binding() {
        let binding = HotkeyBinding::parse("Ctrl+Shift+O").unwrap();
        assert_eq!(binding.canonical(), "CTRL+SHIFT+O");
        assert_eq!(binding.to_string(), "Ctrl+Shift+O");
    }

    #[test]
    fn normalizes_modifier_order_and_case() {
        let a = HotkeyBinding::parse("shift+ctrl+o").unwrap();
        let b = HotkeyBinding::parse("Ctrl+Shift+O").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.canonical(), b.canonical());
    }

    #[test]
    fn accepts_win_alias_for_meta() {
        let a = HotkeyBinding::parse("Win+Alt+R").unwrap();
        let b = HotkeyBinding::parse("Meta+Alt+R").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "Alt+Win+R");
    }

    #[test]
    fn rejects_binding_without_key() {
        assert!(HotkeyBinding::parse("Ctrl+Shift").is_err());
    }

    #[test]
    fn rejects_two_non_modifier_keys() {
        assert!(HotkeyBinding::parse("Ctrl+O+P").is_err());
    }

    #[test]
    fn rejects_empty_accelerator() {
        assert!(HotkeyBinding::parse("").is_err());
        assert!(HotkeyBinding::parse("   ").is_err());
        assert!(HotkeyBinding::parse("Ctrl++O").is_err());
    }

    #[test]
    fn accepts_bare_function_key_without_modifier() {
        let binding = HotkeyBinding::parse("F9").unwrap();
        assert_eq!(binding.canonical(), "F9");
        assert!(binding.modifiers().is_empty());
    }
}
