//! Génération du frontmatter YAML des notes vocales, au format défini par
//! le cahier des charges.

use std::time::Duration;

use chrono::{DateTime, Local};

/// Données nécessaires à la génération du frontmatter d'une note vocale.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontmatterData {
    pub date: DateTime<Local>,
    pub duration: Duration,
    /// Étiquettes supplémentaires, configurées manuellement par
    /// l'utilisateur (la déduction automatique de tags est hors périmètre
    /// de cette itération). `memo-vocal` est toujours ajouté en premier.
    pub extra_tags: Vec<String>,
    /// Chemin relatif au Vault du fichier audio associé, s'il a été copié
    /// dans le Vault (ex. "attachments/rec_20260101_120000.wav").
    pub audio_file: Option<String>,
}

/// Génère le bloc frontmatter YAML (bornes `---` incluses).
pub fn render_frontmatter(data: &FrontmatterData) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("date: {}\n", data.date.format("%Y-%m-%d %H:%M:%S")));
    out.push_str("type: voice-memo\n");
    out.push_str(&format!("duration: {}\n", format_duration_mm_ss(data.duration)));
    out.push_str("tags:\n");
    out.push_str("  - memo-vocal\n");
    for tag in &data.extra_tags {
        if let Some(sanitized) = sanitize_tag(tag) {
            out.push_str(&format!("  - {sanitized}\n"));
        }
    }
    if let Some(audio) = &data.audio_file {
        out.push_str(&format!("audio_file: \"{}\"\n", escape_yaml_string(audio)));
    }
    out.push_str("---\n");
    out
}

/// Formate une durée en `MM:SS`. Les enregistrements visés ne dépassent
/// jamais 99 minutes ; au-delà, les minutes débordent simplement sur plus
/// de deux chiffres plutôt que de paniquer.
fn format_duration_mm_ss(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// Nettoie une étiquette fournie par l'utilisateur : espaces superflus
/// supprimés, retours à la ligne interdits (casseraient le YAML), espaces
/// internes remplacés par des tirets, étiquette vide écartée.
fn sanitize_tag(tag: &str) -> Option<String> {
    let cleaned = tag.replace(['\n', '\r'], " ");
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join("-");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Échappe une chaîne destinée à être insérée entre guillemets doubles dans
/// un scalaire YAML.
fn escape_yaml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_date() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 1, 15, 14, 30, 5).unwrap()
    }

    #[test]
    fn renders_minimal_frontmatter() {
        let data = FrontmatterData {
            date: sample_date(),
            duration: Duration::from_secs(95),
            extra_tags: vec![],
            audio_file: None,
        };
        let rendered = render_frontmatter(&data);
        assert_eq!(
            rendered,
            "---\n\
             date: 2026-01-15 14:30:05\n\
             type: voice-memo\n\
             duration: 01:35\n\
             tags:\n\
             \x20\x20- memo-vocal\n\
             ---\n"
        );
    }

    #[test]
    fn includes_extra_tags_and_audio_file() {
        let data = FrontmatterData {
            date: sample_date(),
            duration: Duration::from_secs(5),
            extra_tags: vec!["réunion".to_string(), "projet x".to_string()],
            audio_file: Some("attachments/rec_20260115_143005.wav".to_string()),
        };
        let rendered = render_frontmatter(&data);
        assert!(rendered.contains("  - memo-vocal\n"));
        assert!(rendered.contains("  - réunion\n"));
        assert!(rendered.contains("  - projet-x\n"));
        assert!(rendered.contains("audio_file: \"attachments/rec_20260115_143005.wav\"\n"));
    }

    #[test]
    fn sanitize_tag_rejects_blank() {
        assert_eq!(sanitize_tag("   "), None);
    }

    #[test]
    fn sanitize_tag_collapses_internal_whitespace() {
        assert_eq!(sanitize_tag("projet   x"), Some("projet-x".to_string()));
    }

    #[test]
    fn duration_formats_with_leading_zeros() {
        assert_eq!(format_duration_mm_ss(Duration::from_secs(65)), "01:05");
        assert_eq!(format_duration_mm_ss(Duration::from_secs(5)), "00:05");
    }
}
