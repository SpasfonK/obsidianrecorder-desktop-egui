//! Écriture de fichiers WAV mono 16 kHz 16 bits : le format exact attendu
//! par les moteurs de reconnaissance vocale hors ligne (Vosk / whisper.cpp)
//! qui seront intégrés en Itération 2.

use std::path::{Path, PathBuf};

use super::dsp::TARGET_SAMPLE_RATE;
use super::error::{AudioError, AudioResult};

/// Spécification WAV imposée pour toute note vocale enregistrée par
/// ObsidianRECORDER Desktop : mono, 16 kHz, PCM 16 bits signé.
pub fn recording_wav_spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

/// Écrivain WAV incrémental : on lui pousse des échantillons PCM 16 bits
/// mono au fur et à mesure qu'ils arrivent du pipeline de capture, puis on
/// le finalise pour obtenir un fichier `.wav` valide et son chemin.
pub struct RecordingWavWriter {
    path: PathBuf,
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    samples_written: u64,
}

impl RecordingWavWriter {
    /// Crée le fichier WAV de destination. Le dossier parent doit déjà
    /// exister : c'est la responsabilité de l'appelant, qui connaît la
    /// politique de rangement des enregistrements (dossier de secours en
    /// Itération 1, Vault Obsidian choisi par l'utilisateur en Itération 3).
    pub fn create(path: impl AsRef<Path>) -> AudioResult<Self> {
        let path = path.as_ref().to_path_buf();
        let writer =
            hound::WavWriter::create(&path, recording_wav_spec()).map_err(|source| {
                AudioError::WavWrite {
                    path: path.clone(),
                    source,
                }
            })?;
        Ok(Self {
            path,
            writer,
            samples_written: 0,
        })
    }

    /// Ajoute des échantillons PCM 16 bits mono déjà ré-échantillonnés à
    /// 16 kHz au fichier en cours d'écriture.
    pub fn write_samples(&mut self, samples: &[i16]) -> AudioResult<()> {
        for &sample in samples {
            self.writer
                .write_sample(sample)
                .map_err(|source| AudioError::WavWrite {
                    path: self.path.clone(),
                    source,
                })?;
        }
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    /// Nombre d'échantillons mono déjà écrits (sert à calculer la durée
    /// affichée dans le frontmatter Markdown à partir de l'Itération 3).
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }

    /// Durée déjà enregistrée, déduite du nombre d'échantillons et de la
    /// fréquence cible de 16 kHz.
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(
            self.samples_written as f64 / TARGET_SAMPLE_RATE as f64,
        )
    }

    /// Finalise l'en-tête WAV (tailles des chunks `data`/`RIFF`) : le
    /// fichier est dès lors lisible par n'importe quel lecteur audio ou
    /// moteur STT.
    pub fn finalize(self) -> AudioResult<PathBuf> {
        let path = self.path.clone();
        self.writer
            .finalize()
            .map_err(|source| AudioError::WavWrite {
                path: path.clone(),
                source,
            })?;
        Ok(path)
    }
}
