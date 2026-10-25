//! Moteur de reconnaissance vocale hors ligne (Whisper, via `whisper-rs` /
//! whisper.cpp, compilé et lié statiquement dans le binaire).
//!
//! Une fois le modèle chargé en mémoire, tout se passe en local : aucun
//! appel réseau. La normalisation textuelle (majuscule initiale,
//! ponctuation finale, espaces superflus) est volontairement simple —
//! affiner la ponctuation est un travail à part entière, hors du périmètre
//! de cette itération.

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::error::{SttError, SttResult};

/// Un segment de transcription, avec ses bornes temporelles dans l'audio
/// source (en centisecondes, unité native de whisper.cpp).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub start_cs: i64,
    pub end_cs: i64,
    pub text: String,
}

/// Transcription complète d'un enregistrement : le détail par segment, plus
/// une vue texte unique concaténée et normalisée.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub segments: Vec<TranscriptSegment>,
}

impl Transcript {
    /// Concatène tous les segments en un texte unique, normalisé.
    pub fn full_text(&self) -> String {
        normalize_text(
            &self
                .segments
                .iter()
                .map(|s| s.text.trim())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// Enveloppe un modèle Whisper chargé, prêt à transcrire de l'audio 16 kHz
/// mono.
///
/// Le chargement (`Transcriber::load`) est coûteux (le modèle est lu en
/// mémoire en entier) : à faire une seule fois au démarrage de
/// l'application, puis réutiliser l'instance pour chaque enregistrement.
/// `WhisperContext` est conçu par whisper-rs pour être partagé (chaque appel
/// à [`Self::transcribe`] crée son propre état de travail via
/// `create_state()`), ce qui permet de l'envelopper dans un `Arc` et de le
/// transmettre entre threads sans verrou dédié.
pub struct Transcriber {
    context: WhisperContext,
    language: String,
}

impl Transcriber {
    /// Charge le modèle Whisper (fichier GGML) depuis `model_path`.
    pub fn load(model_path: impl AsRef<Path>, language: impl Into<String>) -> SttResult<Self> {
        let model_path = model_path.as_ref();
        if !model_path.is_file() {
            return Err(SttError::ModelNotFound(model_path.to_path_buf()));
        }
        let path_str = model_path.to_string_lossy().to_string();

        let context =
            WhisperContext::new_with_params(&path_str, WhisperContextParameters::default())
                .map_err(|e| SttError::ModelLoad {
                    path: model_path.to_path_buf(),
                    message: e.to_string(),
                })?;

        Ok(Self {
            context,
            language: language.into(),
        })
    }

    /// Transcrit un buffer PCM mono 16 kHz déjà normalisé en flottant
    /// [-1.0, 1.0] — exactement le format produit par `audio::dsp` avant
    /// quantification en PCM 16 bits.
    pub fn transcribe(&self, samples: &[f32]) -> SttResult<Transcript> {
        let mut state = self
            .context
            .create_state()
            .map_err(|e| SttError::StateCreation(e.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.language));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, samples)
            .map_err(|e| SttError::Inference(e.to_string()))?;

        let segment_count = state
            .full_n_segments()
            .map_err(|e| SttError::Inference(e.to_string()))?;

        let mut segments = Vec::with_capacity(segment_count.max(0) as usize);
        for i in 0..segment_count {
            let text = state
                .full_get_segment_text(i)
                .map_err(|e| SttError::Inference(e.to_string()))?;
            let start_cs = state.full_get_segment_t0(i).unwrap_or(0) as i64;
            let end_cs = state.full_get_segment_t1(i).unwrap_or(0) as i64;
            segments.push(TranscriptSegment {
                start_cs,
                end_cs,
                text: normalize_text(&text),
            });
        }

        Ok(Transcript { segments })
    }

    /// Lit un fichier `.wav` mono 16 kHz PCM 16 bits (format produit par
    /// `audio::wav_writer::RecordingWavWriter`) et le transcrit directement.
    pub fn transcribe_wav_file(&self, wav_path: impl AsRef<Path>) -> SttResult<Transcript> {
        let mut reader = hound::WavReader::open(wav_path.as_ref())
            .map_err(|e| SttError::AudioRead(e.to_string()))?;
        let spec = reader.spec();
        if spec.channels != 1 || spec.sample_rate != 16_000 {
            return Err(SttError::AudioRead(format!(
                "format WAV inattendu (attendu mono 16 kHz, obtenu {} canal(aux) à {} Hz)",
                spec.channels, spec.sample_rate
            )));
        }

        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()
            .map_err(|e| SttError::AudioRead(e.to_string()))?;

        self.transcribe(&samples)
    }
}

/// Normalisation textuelle basique : espaces superflus supprimés, première
/// lettre en majuscule, ponctuation finale garantie.
fn normalize_text(text: &str) -> String {
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return trimmed;
    }

    let mut chars = trimmed.chars();
    let mut capitalized: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };

    if !matches!(capitalized.chars().last(), Some('.' | '!' | '?' | '…')) {
        capitalized.push('.');
    }

    capitalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_capitalizes() {
        assert_eq!(normalize_text("  bonjour   le monde  "), "Bonjour le monde.");
    }

    #[test]
    fn normalize_keeps_existing_punctuation() {
        assert_eq!(normalize_text("comment ça va ?"), "Comment ça va ?");
    }

    #[test]
    fn normalize_empty_stays_empty() {
        assert_eq!(normalize_text("   "), "");
    }

    #[test]
    fn normalize_handles_single_character() {
        assert_eq!(normalize_text("a"), "A.");
    }

    #[test]
    fn transcript_full_text_joins_segments() {
        let transcript = Transcript {
            segments: vec![
                TranscriptSegment {
                    start_cs: 0,
                    end_cs: 100,
                    text: "Bonjour".into(),
                },
                TranscriptSegment {
                    start_cs: 100,
                    end_cs: 250,
                    text: "le monde".into(),
                },
            ],
        };
        assert_eq!(transcript.full_text(), "Bonjour le monde.");
    }

    #[test]
    fn transcript_full_text_empty_when_no_segments() {
        let transcript = Transcript::default();
        assert_eq!(transcript.full_text(), "");
    }
}
