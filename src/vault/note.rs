//! Assemblage de la note Markdown complète (frontmatter + corps), au format
//! exact défini par le cahier des charges.

use super::frontmatter::{render_frontmatter, FrontmatterData};

/// Contenu nécessaire à la génération d'une note vocale complète.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteData {
    pub frontmatter: FrontmatterData,
    pub transcript_text: String,
}

/// Génère le contenu Markdown complet d'une note vocale : frontmatter YAML,
/// titre daté, lecture intégrée du (des) fichier(s) audio, puis citation
/// contenant la transcription.
pub fn render_note(data: &NoteData) -> String {
    let mut out = render_frontmatter(&data.frontmatter);
    out.push('\n');
    out.push_str(&format!(
        "# Note vocale du {}\n\n",
        data.frontmatter.date.format("%d/%m/%Y")
    ));
    // Lecture directe dans Obsidian (un enregistrement découpé en produit
    // plusieurs).
    for audio in &data.frontmatter.audio_files {
        out.push_str(&format!("![[{audio}]]\n"));
    }
    if !data.frontmatter.audio_files.is_empty() {
        out.push('\n');
    }
    out.push_str("> Résumé / Transcription :\n");
    out.push_str(&data.transcript_text);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::time::Duration;

    #[test]
    fn renders_full_note_with_expected_sections() {
        let data = NoteData {
            frontmatter: FrontmatterData {
                date: chrono::Local.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap(),
                duration: Duration::from_secs(12),
                extra_tags: vec![],
                audio_files: vec!["attachments/rec_x.mp3".to_string()],
            },
            transcript_text: "Bonjour, ceci est un test.".to_string(),
        };

        let note = render_note(&data);

        assert!(note.starts_with("---\n"));
        assert!(note.contains("# Note vocale du 02/03/2026\n"));
        assert!(note.contains("![[attachments/rec_x.mp3]]\n"));
        assert!(note.contains("> Résumé / Transcription :\n"));
        assert!(note.ends_with("Bonjour, ceci est un test.\n"));
    }

    #[test]
    fn renders_a_note_without_audio_embed_when_no_file() {
        let data = NoteData {
            frontmatter: FrontmatterData {
                date: chrono::Local.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap(),
                duration: Duration::from_secs(3),
                extra_tags: vec![],
                audio_files: vec![],
            },
            transcript_text: "Bonjour.".to_string(),
        };
        let note = render_note(&data);
        assert!(!note.contains("![["), "aucun embed attendu sans fichier");
    }
}
