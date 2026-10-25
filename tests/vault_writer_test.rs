//! Test d'intégration correspondant aux critères du jalon 3 : « Test
//! unitaire de génération YAML et sérialisation Markdown » et « Test de
//! concurrence : écriture pendant la modification d'un fichier ».
//!
//! Les cas limites détaillés (normalisation des tags, formats de durée,
//! conflits de nommage, concurrence) vivent en tests unitaires directement
//! dans `src/vault/frontmatter.rs`, `note.rs` et `writer.rs`. Celui-ci
//! exerce le pipeline complet bout en bout, comme le ferait l'application
//! réelle : ouverture d'un Vault, génération de la note, écriture sur le
//! disque dans les deux modes (nouvelle note / Daily Note).

use std::time::Duration;

use obsidianrecorder_lib::vault::{write_note, FrontmatterData, NoteData, Vault, WriteMode};

fn make_vault(tmp: &std::path::Path) -> Vault {
    std::fs::create_dir_all(tmp.join(".obsidian")).unwrap();
    Vault::open(tmp).expect("le dossier préparé doit être reconnu comme un Vault valide")
}

#[test]
fn full_pipeline_renders_valid_markdown_and_writes_it_to_the_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = make_vault(tmp.path());

    let note = NoteData {
        frontmatter: FrontmatterData {
            date: chrono::Local::now(),
            duration: Duration::from_secs(42),
            extra_tags: vec!["réunion".to_string()],
            audio_file: Some("attachments/rec_test.wav".to_string()),
        },
        transcript_text: "Ceci est une transcription de test.".to_string(),
    };

    let content = obsidianrecorder_lib::vault::note::render_note(&note);

    // Génération YAML / Markdown : structure attendue, dans le bon ordre.
    assert!(content.starts_with("---\n"));
    assert!(content.contains("type: voice-memo\n"));
    assert!(content.contains("duration: 00:42\n"));
    assert!(content.contains("  - memo-vocal\n"));
    assert!(content.contains("  - réunion\n"));
    assert!(content.contains("audio_file: \"attachments/rec_test.wav\"\n"));
    assert!(content.contains("Ceci est une transcription de test."));

    let target_dir = vault.resolve("AudioNotes");
    let written_path = write_note(
        &WriteMode::NewFile {
            target_dir: target_dir.clone(),
        },
        "note-test.md",
        &content,
    )
    .expect("l'écriture doit réussir");

    assert_eq!(written_path, target_dir.join("note-test.md"));
    assert_eq!(std::fs::read_to_string(&written_path).unwrap(), content);
}

#[test]
fn append_to_daily_note_mode_accumulates_sections() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = make_vault(tmp.path());
    let daily_notes_dir = vault.resolve("Daily Notes");

    let mode = WriteMode::AppendToDailyNote {
        daily_notes_dir: daily_notes_dir.clone(),
    };

    let first = write_note(&mode, "ignoré-pour-ce-mode.md", "Section A").unwrap();
    let second = write_note(&mode, "ignoré-pour-ce-mode.md", "Section B").unwrap();

    assert_eq!(
        first, second,
        "les deux appels du jour doivent cibler la même Daily Note"
    );
    let content = std::fs::read_to_string(&first).unwrap();
    assert_eq!(content, "Section A\n\nSection B");
}

#[test]
fn naming_conflicts_never_overwrite_an_existing_note() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = make_vault(tmp.path());
    let target_dir = vault.resolve("AudioNotes");
    let mode = WriteMode::NewFile {
        target_dir: target_dir.clone(),
    };

    let first = write_note(&mode, "memo.md", "premier contenu").unwrap();
    let second = write_note(&mode, "memo.md", "second contenu").unwrap();

    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "premier contenu");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "second contenu");
}
