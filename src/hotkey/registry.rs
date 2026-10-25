//! Registre « pur » des raccourcis clavier globaux : associe un
//! identifiant interne à un [`HotkeyBinding`] et une action, et refuse
//! toute tentative d'enregistrer deux fois le même raccourci physique.
//!
//! Ce module ne dépend d'aucune API système : il est donc entièrement
//! testable sans avoir à enregistrer de vrai raccourci auprès de Windows.
//! L'enregistrement réel auprès de l'OS vit dans [`super::os_backend`].

use std::collections::HashMap;

use super::binding::HotkeyBinding;
use super::error::{HotkeyError, HotkeyResult};

/// Identifiant opaque attribué par le registre à chaque raccourci
/// enregistré avec succès. Ne peut être construit que par le registre
/// lui-même.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HotkeyId(u32);

/// Action déclenchée par un raccourci global.
///
/// L'Itération 1 ne couvre que le démarrage/l'arrêt de l'enregistrement ;
/// d'autres actions pourront être ajoutées sans changer la logique de
/// détection de collision ci-dessous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleRecording,
    StartRecording,
    StopRecording,
}

#[derive(Debug)]
struct Entry {
    binding: HotkeyBinding,
    action: HotkeyAction,
}

/// Registre en mémoire des raccourcis actuellement enregistrés.
#[derive(Debug)]
pub struct HotkeyRegistry {
    entries: HashMap<HotkeyId, Entry>,
    next_id: u32,
}

impl HotkeyRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 1,
        }
    }

    /// Tente d'enregistrer un raccourci pour une action donnée.
    ///
    /// Retourne [`HotkeyError::AlreadyBound`] si le même raccourci (après
    /// normalisation) est déjà associé à une autre entrée du registre :
    /// aucune collision silencieuse n'est possible.
    pub fn register(
        &mut self,
        binding: HotkeyBinding,
        action: HotkeyAction,
    ) -> HotkeyResult<HotkeyId> {
        if let Some(existing) = self.find_by_binding(&binding) {
            return Err(HotkeyError::AlreadyBound(existing.binding.to_string()));
        }

        let id = HotkeyId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.entries.insert(id, Entry { binding, action });
        Ok(id)
    }

    /// Retire un raccourci du registre, libérant l'accélérateur pour un
    /// futur enregistrement.
    pub fn unregister(&mut self, id: HotkeyId) -> HotkeyResult<()> {
        self.entries
            .remove(&id)
            .map(|_| ())
            .ok_or(HotkeyError::UnknownBinding(id.0))
    }

    pub fn is_registered(&self, binding: &HotkeyBinding) -> bool {
        self.find_by_binding(binding).is_some()
    }

    pub fn action_for(&self, id: HotkeyId) -> Option<HotkeyAction> {
        self.entries.get(&id).map(|e| e.action)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn find_by_binding(&self, binding: &HotkeyBinding) -> Option<&Entry> {
        self.entries.values().find(|e| &e.binding == binding)
    }
}

impl Default for HotkeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(accelerator: &str) -> HotkeyBinding {
        HotkeyBinding::parse(accelerator).unwrap()
    }

    #[test]
    fn registers_and_unregisters_without_collision() {
        let mut registry = HotkeyRegistry::new();
        let id = registry
            .register(binding("Ctrl+Shift+O"), HotkeyAction::ToggleRecording)
            .expect("le premier enregistrement doit réussir");

        assert!(registry.is_registered(&binding("Ctrl+Shift+O")));
        assert_eq!(registry.len(), 1);

        registry.unregister(id).expect("la désinscription doit réussir");
        assert!(!registry.is_registered(&binding("Ctrl+Shift+O")));
        assert!(registry.is_empty());
    }

    #[test]
    fn rejects_duplicate_binding_even_with_different_casing() {
        let mut registry = HotkeyRegistry::new();
        registry
            .register(binding("Ctrl+Shift+O"), HotkeyAction::ToggleRecording)
            .unwrap();

        let collision = registry.register(binding("shift+ctrl+o"), HotkeyAction::StartRecording);
        assert!(matches!(collision, Err(HotkeyError::AlreadyBound(_))));
        assert_eq!(registry.len(), 1, "une collision ne doit rien ajouter au registre");
    }

    #[test]
    fn allows_rebinding_after_unregister() {
        let mut registry = HotkeyRegistry::new();
        let id = registry
            .register(binding("Win+Alt+R"), HotkeyAction::ToggleRecording)
            .unwrap();
        registry.unregister(id).unwrap();

        let new_id = registry
            .register(binding("Win+Alt+R"), HotkeyAction::StopRecording)
            .expect("le raccourci libéré doit pouvoir être réutilisé");
        assert_ne!(id, new_id);
        assert_eq!(registry.action_for(new_id), Some(HotkeyAction::StopRecording));
    }

    #[test]
    fn unregistering_unknown_id_returns_error() {
        let mut registry = HotkeyRegistry::new();
        let id = registry
            .register(binding("Ctrl+Shift+O"), HotkeyAction::ToggleRecording)
            .unwrap();
        registry.unregister(id).unwrap();

        // Le même identifiant, déjà retiré, ne peut pas l'être une seconde fois.
        assert!(matches!(
            registry.unregister(id),
            Err(HotkeyError::UnknownBinding(_))
        ));
    }

    #[test]
    fn distinct_bindings_can_coexist() {
        let mut registry = HotkeyRegistry::new();
        registry
            .register(binding("Ctrl+Shift+O"), HotkeyAction::ToggleRecording)
            .unwrap();
        registry
            .register(binding("Win+Alt+R"), HotkeyAction::StartRecording)
            .unwrap();
        assert_eq!(registry.len(), 2);
    }
}
