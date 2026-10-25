//! Récapitulatif des notes vocales déjà écrites dans le Vault : liste les
//! fichiers `.md` d'un dossier (typiquement `<Vault>/AudioNotes`), avec un
//! minimum d'informations extraites du frontmatter (date, durée).
//!
//! Le frontmatter est entièrement généré par cette application (voir
//! [`super::frontmatter::render_frontmatter`]) : sa structure est connue et
//! stable, ce qui permet une simple lecture ligne à ligne plutôt que
//! d'ajouter une dépendance à un analyseur YAML complet pour deux champs
//! scalaires.
//!
//! Ne couvre que le dossier des nouvelles notes horodatées : les entrées
//! ajoutées en mode Daily Note vivent dans un fichier partagé par jour
//! (`Daily Notes/YYYY-MM-DD.md`), sans correspondance 1 pour 1 avec un
//! enregistrement audio — elles ne sont donc volontairement pas listées
//! ici.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Résumé d'une note vocale déjà écrite, pour l'affichage dans le
/// dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSummary {
    pub path: PathBuf,
    pub date_label: Option<String>,
    pub duration_label: Option<String>,
    pub modified: Option<SystemTime>,
}

/// Liste les notes vocales du dossier donné, triées des plus récentes aux
/// plus anciennes (par date de modification du fichier), limitées à
/// `limit` entrées. Retourne une liste vide si le dossier n'existe pas
/// encore (ex. aucune note écrite pour le moment) plutôt qu'une erreur.
pub fn list_recent_notes(dir: &Path, limit: usize) -> Vec<NoteSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut notes: Vec<NoteSummary> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("md"))
        .map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
            let (date_label, duration_label) = std::fs::read_to_string(&path)
                .map(|content| extract_frontmatter_fields(&content))
                .unwrap_or((None, None));
            NoteSummary {
                path,
                date_label,
                duration_label,
                modified,
            }
        })
        .collect();

    notes.sort_by(|a, b| b.modified.cmp(&a.modified));
    notes.truncate(limit);
    notes
}

/// Extrait les champs `date:` et `duration:` du frontmatter YAML généré
/// par cette application, par lecture ligne à ligne.
fn extract_frontmatter_fields(content: &str) -> (Option<String>, Option<String>) {
    let mut date = None;
    let mut duration = None;
    for line in content.lines().take(20) {
        if let Some(value) = line.strip_prefix("date: ") {
            date = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("duration: ") {
            duration = Some(value.trim().to_string());
        }
        if line.trim() == "---" && date.is_some() {
            break;
        }
    }
    (date, duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_date_and_duration_from_known_frontmatter_format() {
        let content = "---\ndate: 2026-01-15 14:30:05\ntype: voice-memo\nduration: 01:35\ntags:\n  - memo-vocal\n---\n# Note\n";
        let (date, duration) = extract_frontmatter_fields(content);
        assert_eq!(date, Some("2026-01-15 14:30:05".to_string()));
        assert_eq!(duration, Some("01:35".to_string()));
    }

    #[test]
    fn returns_none_fields_when_absent() {
        let (date, duration) = extract_frontmatter_fields("pas de frontmatter ici");
        assert_eq!(date, None);
        assert_eq!(duration, None);
    }

    #[test]
    fn list_recent_notes_returns_empty_for_missing_directory() {
        let notes = list_recent_notes(
            Path::new("/dossier/qui/n_existe/vraiment/pas/obsidianrecorder"),
            10,
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn list_recent_notes_ignores_non_markdown_files_and_respects_limit() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(
                tmp.path().join(format!("note-{i}.md")),
                format!("---\ndate: 2026-01-{:02} 10:00:00\nduration: 00:1{i}\n---\n", i + 1),
            )
            .unwrap();
        }
        std::fs::write(tmp.path().join("attachment.wav"), b"pas un .md").unwrap();

        let notes = list_recent_notes(tmp.path(), 3);
        assert_eq!(notes.len(), 3);
        assert!(notes.iter().all(|n| n.path.extension().unwrap() == "md"));
    }
}
