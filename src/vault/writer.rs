//! Écriture sécurisée des notes vocales dans le Vault : atomique (pour ne
//! jamais exposer à Obsidian un fichier à moitié écrit), avec gestion des
//! conflits de nommage et des deux modes requis par le cahier des charges
//! (nouvelle note horodatée / ajout à la Daily Note du jour).

use std::io::Write;
use std::path::{Path, PathBuf};

use super::error::{VaultError, VaultResult};

/// Mode d'écriture d'une note vocale.
#[derive(Debug, Clone)]
pub enum WriteMode {
    /// Crée un nouveau fichier horodaté dans `target_dir` (sous-dossier du
    /// Vault, ex. "AudioNotes" ou "Inbox").
    NewFile { target_dir: PathBuf },
    /// Ajoute une section à la fin de la Daily Note du jour, dans
    /// `daily_notes_dir` (créée si elle n'existe pas encore). Le nom de
    /// fichier passé à [`write_note`] est ignoré dans ce mode : le nom de
    /// la Daily Note (`YYYY-MM-DD.md`) est toujours dérivé de la date du
    /// jour.
    AppendToDailyNote { daily_notes_dir: PathBuf },
}

/// Écrit une note vocale selon le mode demandé et retourne le chemin du
/// fichier `.md` final.
pub fn write_note(mode: &WriteMode, base_filename: &str, content: &str) -> VaultResult<PathBuf> {
    match mode {
        WriteMode::NewFile { target_dir } => {
            write_new_file_atomic(target_dir, base_filename, content)
        }
        WriteMode::AppendToDailyNote { daily_notes_dir } => {
            append_to_daily_note(daily_notes_dir, content)
        }
    }
}

/// Crée un nouveau fichier dans `target_dir`, sans jamais écraser un
/// fichier existant (voir [`unique_path`]), et l'écrit de façon atomique.
fn write_new_file_atomic(
    target_dir: &Path,
    base_filename: &str,
    content: &str,
) -> VaultResult<PathBuf> {
    std::fs::create_dir_all(target_dir)?;
    let final_path = unique_path(target_dir, base_filename);
    atomic_write(&final_path, content)?;
    Ok(final_path)
}

/// Trouve un nom de fichier disponible dans `dir`, en ajoutant un suffixe
/// numérique (`-1`, `-2`, ...) en cas de conflit — jamais d'écrasement
/// silencieux d'une note déjà présente.
fn unique_path(dir: &Path, base_filename: &str) -> PathBuf {
    let candidate = dir.join(base_filename);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(base_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("note");
    let ext = Path::new(base_filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("md");

    for n in 1..1000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    // Filet de secours extrême (plus de 999 collisions le même jour) : un
    // horodatage en nanosecondes élimine toute collision possible.
    dir.join(format!(
        "{stem}-{}.{ext}",
        chrono::Local::now().format("%Y%m%d%H%M%S%f")
    ))
}

/// Ajoute `section` à la fin de la Daily Note du jour (créée si elle
/// n'existe pas encore), séparée du contenu existant par une ligne vide,
/// puis réécrit le fichier entier de façon atomique.
fn append_to_daily_note(daily_notes_dir: &Path, section: &str) -> VaultResult<PathBuf> {
    std::fs::create_dir_all(daily_notes_dir)?;
    let filename = format!("{}.md", chrono::Local::now().format("%Y-%m-%d"));
    let path = daily_notes_dir.join(filename);

    let mut content = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };

    if !content.is_empty() {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
    }
    content.push_str(section);

    atomic_write(&path, &content)?;
    Ok(path)
}

/// Écriture atomique : le contenu est d'abord écrit dans un fichier
/// temporaire du même dossier (même volume, condition requise pour que le
/// renommage soit atomique), puis renommé par-dessus la destination finale
/// (`tempfile::NamedTempFile::persist`, qui remplace atomiquement un
/// fichier existant, y compris sous Windows via `MoveFileExW`). Un
/// indexeur de fichiers (Obsidian, antivirus, etc.) qui observerait ce
/// dossier ne voit donc jamais un fichier à moitié écrit : seulement
/// l'ancienne version complète, ou la nouvelle.
fn atomic_write(path: &Path, content: &str) -> VaultResult<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".obsidianrecorder-tmp-")
        .suffix(".md")
        .tempfile_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| VaultError::Io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn write_new_file_creates_expected_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_new_file_atomic(tmp.path(), "note.md", "contenu de test").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "contenu de test");
        assert_eq!(path, tmp.path().join("note.md"));
    }

    #[test]
    fn write_new_file_avoids_overwriting_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let first = write_new_file_atomic(tmp.path(), "note.md", "premier").unwrap();
        let second = write_new_file_atomic(tmp.path(), "note.md", "second").unwrap();

        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "premier");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
        assert_eq!(second, tmp.path().join("note-1.md"));
    }

    #[test]
    fn append_to_daily_note_creates_file_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = append_to_daily_note(tmp.path(), "Première section").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Première section");
    }

    #[test]
    fn append_to_daily_note_adds_blank_line_between_sections() {
        let tmp = tempfile::tempdir().unwrap();
        append_to_daily_note(tmp.path(), "Section 1").unwrap();
        let path = append_to_daily_note(tmp.path(), "Section 2").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "Section 1\n\nSection 2");
    }

    /// Gauntlet Test : « Test de concurrence : écriture pendant la
    /// modification d'un fichier ». Un thread lit le fichier en boucle
    /// pendant qu'un autre le réécrit (atomiquement) de façon répétée ; à
    /// aucun moment une lecture ne doit renvoyer un contenu tronqué ou
    /// mixte — seulement l'une des deux versions complètes.
    #[test]
    fn concurrent_reads_never_observe_a_torn_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("concurrent.md");
        // Version initiale, pour que le lecteur ait toujours quelque chose
        // de valide à lire dès le début.
        atomic_write(&path, "VERSION-A").unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader_path = path.clone();
        let reader_stop = Arc::clone(&stop);

        let reader = std::thread::spawn(move || {
            let mut observations = 0u32;
            while !reader_stop.load(Ordering::Relaxed) {
                if let Ok(content) = std::fs::read_to_string(&reader_path) {
                    assert!(
                        content == "VERSION-A" || content == "VERSION-B",
                        "lecture corrompue : {content:?}"
                    );
                    observations += 1;
                }
            }
            observations
        });

        for i in 0..200 {
            let content = if i % 2 == 0 { "VERSION-B" } else { "VERSION-A" };
            atomic_write(&path, content).unwrap();
        }

        stop.store(true, Ordering::Relaxed);
        let observations = reader.join().unwrap();
        // Le lecteur doit avoir eu l'occasion d'observer plusieurs lectures
        // pendant la phase d'écriture, sinon le test ne prouverait rien.
        assert!(observations > 0, "le thread lecteur n'a rien pu lire");
    }

    #[test]
    fn unique_path_falls_back_after_many_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("note.md"), "x").unwrap();
        for n in 1..5 {
            std::fs::write(tmp.path().join(format!("note-{n}.md")), "x").unwrap();
        }
        let path = unique_path(tmp.path(), "note.md");
        assert_eq!(path, tmp.path().join("note-5.md"));
    }
}
