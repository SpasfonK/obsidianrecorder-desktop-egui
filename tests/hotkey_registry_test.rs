//! Test d'intégration correspondant au critère du jalon 1 : « Test unitaire
//! d'enregistrement/désinscription des hotkeys sans collision ».

use obsidianrecorder_lib::hotkey::{HotkeyAction, HotkeyBinding, HotkeyError, HotkeyRegistry};

#[test]
fn full_lifecycle_register_collision_unregister_rebind() {
    let mut registry = HotkeyRegistry::new();

    // 1. Enregistrement des deux raccourcis par défaut du cahier des charges.
    let toggle = registry
        .register(
            HotkeyBinding::parse("Ctrl+Shift+O").unwrap(),
            HotkeyAction::ToggleRecording,
        )
        .expect("le raccourci de bascule doit s'enregistrer sans collision");

    let start = registry
        .register(
            HotkeyBinding::parse("Win+Alt+R").unwrap(),
            HotkeyAction::StartRecording,
        )
        .expect("un second raccourci distinct doit s'enregistrer sans collision");

    assert_ne!(toggle, start);
    assert_eq!(registry.len(), 2);

    // 2. Toute tentative de ré-enregistrer un raccourci déjà pris est
    //    rejetée, y compris avec une écriture différente (collision).
    let collision = registry.register(
        HotkeyBinding::parse("shift+ctrl+o").unwrap(),
        HotkeyAction::StopRecording,
    );
    assert!(matches!(collision, Err(HotkeyError::AlreadyBound(_))));
    assert_eq!(
        registry.len(),
        2,
        "une collision ne doit rien modifier au registre"
    );

    // 3. La désinscription libère le raccourci.
    registry
        .unregister(toggle)
        .expect("la désinscription doit réussir");
    assert_eq!(registry.len(), 1);

    // 4. Le raccourci libéré peut être ré-enregistré pour une autre action.
    let rebound = registry
        .register(
            HotkeyBinding::parse("Ctrl+Shift+O").unwrap(),
            HotkeyAction::StopRecording,
        )
        .expect("un raccourci libéré doit pouvoir être ré-enregistré");
    assert_ne!(rebound, toggle);
    assert_eq!(registry.action_for(rebound), Some(HotkeyAction::StopRecording));

    // 5. Désinscrire un identifiant déjà retiré échoue proprement.
    assert!(matches!(
        registry.unregister(toggle),
        Err(HotkeyError::UnknownBinding(_))
    ));

    // 6. Le second raccourci, jamais touché, est toujours actif et distinct.
    assert_eq!(registry.action_for(start), Some(HotkeyAction::StartRecording));
    assert_eq!(registry.len(), 2);
}
