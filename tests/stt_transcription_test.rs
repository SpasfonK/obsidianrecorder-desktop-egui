//! Tests d'intégration correspondant aux critères de l'itération 2 :
//! « Injection de fichiers audio WAV de référence (mots de test en
//! français) et assertion sur le texte transcrit » et « Mesure des fuites
//! mémoire et libération correcte du recognizer ».
//!
//! Les deux tests ci-dessous nécessitent des éléments que je ne peux pas
//! produire moi-même dans mon environnement d'exécution (pas de micro, pas
//! de moteur de synthèse vocale, pas d'accès réseau) :
//! 1. le modèle Whisper GGML (voir `scripts/download-model.ps1`) ;
//! 2. pour le premier test, un fichier audio de référence contenant une
//!    phrase française connue (voir `tests/fixtures/README.md`).
//!
//! Ils sont donc `#[ignore]` par défaut : `cargo test` (utilisé en CI) reste
//! rapide et ne télécharge jamais le modèle. Pour les exécuter localement,
//! une fois le modèle et le fichier de référence en place :
//!
//! ```text
//! cargo test -- --ignored
//! ```
//!
//! Chacun se désactive aussi proprement (message explicatif, pas d'échec)
//! si l'un des deux prérequis manque encore.

use std::path::Path;

use obsidianrecorder_lib::stt::model::{default_model_is_present, default_model_path};
use obsidianrecorder_lib::stt::Transcriber;

const FIXTURE_PATH: &str = "tests/fixtures/bonjour_ceci_est_un_test.wav";

/// Mots-clés attendus dans la transcription (comparaison insensible à la
/// casse) : on vérifie leur présence plutôt qu'une égalité stricte, la
/// sortie exacte de Whisper pouvant varier légèrement selon la voix, le
/// débit et le bruit ambiant de l'enregistrement fourni.
const EXPECTED_KEYWORDS: &[&str] = &["bonjour", "test"];

#[test]
#[ignore = "nécessite le modèle Whisper téléchargé et un fichier audio de référence — voir tests/fixtures/README.md"]
fn transcribes_reference_french_audio() {
    if !default_model_is_present() {
        eprintln!(
            "Test ignoré : modèle Whisper absent ({}). Lancez scripts/download-model.ps1.",
            default_model_path().display()
        );
        return;
    }

    if !Path::new(FIXTURE_PATH).is_file() {
        eprintln!(
            "Test ignoré : fichier de référence absent ({FIXTURE_PATH}). Voir tests/fixtures/README.md."
        );
        return;
    }

    let transcriber =
        Transcriber::load(default_model_path(), "fr").expect("le modèle doit se charger correctement");
    let transcript = transcriber
        .transcribe_wav_file(FIXTURE_PATH)
        .expect("la transcription doit réussir");

    let text_lower = transcript.full_text().to_lowercase();
    for keyword in EXPECTED_KEYWORDS {
        assert!(
            text_lower.contains(keyword),
            "mot-clé « {keyword} » absent de la transcription obtenue : « {} »",
            transcript.full_text()
        );
    }
}

#[test]
#[ignore = "nécessite le modèle Whisper téléchargé — mesure la stabilité du recognizer"]
fn repeated_load_transcribe_and_drop_does_not_crash() {
    // Sans instrumentation externe (valgrind/heaptrack, indisponibles ici),
    // la vérification pragmatique en Rust d'une « libération correcte du
    // recognizer » est : charger, transcrire et détruire le contexte
    // plusieurs fois de suite ne doit jamais paniquer. Le destructeur
    // (RAII) de `WhisperContext` est responsable de la libération côté
    // whisper.cpp ; ce test exerce ce chemin de façon répétée.
    if !default_model_is_present() {
        eprintln!(
            "Test ignoré : modèle Whisper absent ({}).",
            default_model_path().display()
        );
        return;
    }

    let silence = vec![0.0_f32; 16_000]; // 1 s de silence : suffisant pour exercer tout le pipeline
    for iteration in 0..3 {
        let transcriber = Transcriber::load(default_model_path(), "fr")
            .unwrap_or_else(|e| panic!("chargement du modèle (itération {iteration}) : {e}"));
        let _ = transcriber.transcribe(&silence);
        drop(transcriber);
    }
}
