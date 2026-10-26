//! Lecture audio d'aperçu : lit un enregistrement (`.wav` ou `.mp3`, via
//! [`super::import::decode_to_mono`]) et le restitue sur le périphérique de
//! sortie par défaut.
//!
//! Même contrainte que `audio::capture` : `cpal::Stream` n'implémente pas
//! `Send` sous Windows (WASAPI/COM) — même solution, un thread dédié qui
//! crée, garde en vie puis détruit le flux. Le pipeline est le miroir de
//! la capture : au lieu de downmix + resample + quantifier + écrire, on
//! ré-échantillonne vers la fréquence native du périphérique de sortie,
//! puis on dépose chaque échantillon mono sur tous les canaux de sortie
//! (upmix simple, suffisant pour une prévisualisation).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::dsp::resample_linear;
use super::error::{AudioError, AudioResult};
use super::import::decode_to_mono;

enum PlaybackCommand {
    Stop,
}

/// Pilote la lecture audio de prévisualisation.
pub struct AudioPlayback {
    control_tx: Option<mpsc::Sender<PlaybackCommand>>,
    worker: Option<thread::JoinHandle<AudioResult<()>>>,
}

impl AudioPlayback {
    pub fn new() -> Self {
        Self {
            control_tx: None,
            worker: None,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.worker.is_some()
    }

    /// Démarre la lecture de `audio_path` (`.wav`, `.mp3` ou `.m4a`).
    /// Bloque brièvement le temps que le thread dédié confirme que le flux
    /// audio a bien démarré.
    pub fn play(&mut self, audio_path: impl AsRef<Path>) -> AudioResult<()> {
        if self.worker.is_some() {
            return Err(AudioError::AlreadyPlaying);
        }

        let audio_path = audio_path.as_ref().to_path_buf();
        let (control_tx, control_rx) = mpsc::channel::<PlaybackCommand>();
        let (ready_tx, ready_rx) = mpsc::channel::<AudioResult<()>>();

        let worker = thread::Builder::new()
            .name("obsidianrecorder-audio-playback".into())
            .spawn(move || run_playback_thread(audio_path, control_rx, ready_tx))
            .map_err(|e| AudioError::BuildStream(e.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.control_tx = Some(control_tx);
                self.worker = Some(worker);
                Ok(())
            }
            Ok(Err(err)) => {
                let _ = worker.join();
                Err(err)
            }
            Err(_) => {
                let _ = worker.join();
                Err(AudioError::BuildStream(
                    "le thread de lecture s'est arrêté avant d'avoir démarré le flux audio".into(),
                ))
            }
        }
    }

    /// Arrête la lecture en cours.
    pub fn stop(&mut self) -> AudioResult<()> {
        let control_tx = self.control_tx.take().ok_or(AudioError::NotPlaying)?;
        let worker = self.worker.take().ok_or(AudioError::NotPlaying)?;
        let _ = control_tx.send(PlaybackCommand::Stop);
        worker
            .join()
            .map_err(|_| AudioError::BuildStream("le thread de lecture a paniqué".into()))?
    }

    /// À appeler régulièrement (ex. à chaque frame de l'interface) : si la
    /// lecture s'est terminée naturellement (fin du fichier) depuis le
    /// dernier appel, nettoie la poignée du thread et retourne `true`.
    pub fn poll_finished(&mut self) -> bool {
        let finished = self
            .worker
            .as_ref()
            .map(thread::JoinHandle::is_finished)
            .unwrap_or(false);
        if finished {
            self.control_tx.take();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
        finished
    }
}

impl Default for AudioPlayback {
    fn default() -> Self {
        Self::new()
    }
}

fn run_playback_thread(
    audio_path: PathBuf,
    control_rx: mpsc::Receiver<PlaybackCommand>,
    ready_tx: mpsc::Sender<AudioResult<()>>,
) -> AudioResult<()> {
    let setup = setup_playback_stream(&audio_path);
    let (stream, finished_rx) = match setup {
        Ok(pair) => {
            let _ = ready_tx.send(Ok(()));
            pair
        }
        Err(err) => {
            let _ = ready_tx.send(Err(AudioError::BuildStream(err.to_string())));
            return Err(err);
        }
    };

    // On attend soit un ordre d'arrêt explicite, soit la fin naturelle de
    // la lecture (signalée par `finished_rx`), en gardant `stream` en vie
    // sur ce thread jusque-là (voir note de module).
    loop {
        match control_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(_) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if finished_rx.try_recv().is_ok() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(stream);
    Ok(())
}

fn setup_playback_stream(
    audio_path: &Path,
) -> AudioResult<(cpal::Stream, mpsc::Receiver<()>)> {
    // Décode aussi bien le WAV que le MP3/M4A (voir `super::import`).
    let (source_mono, source_rate) = decode_to_mono(audio_path)?;

    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoOutputDevice)?;
    let supported_config = device
        .default_output_config()
        .map_err(|e| AudioError::ConfigQuery(e.to_string()))?;
    let sample_format = supported_config.sample_format();
    let output_rate = supported_config.sample_rate().0;
    let output_channels = supported_config.channels();
    let stream_config: cpal::StreamConfig = supported_config.into();

    let resampled = if source_rate == output_rate {
        source_mono
    } else {
        resample_linear(&source_mono, source_rate, output_rate)
    };
    let samples = Arc::new(resampled);
    let position = Arc::new(AtomicUsize::new(0));
    let (finished_tx, finished_rx) = mpsc::channel();

    let stream = build_output_stream(
        &device,
        &stream_config,
        sample_format,
        samples,
        position,
        output_channels,
        finished_tx,
    )?;
    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))?;

    Ok((stream, finished_rx))
}

fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    samples: Arc<Vec<f32>>,
    position: Arc<AtomicUsize>,
    output_channels: u16,
    finished_tx: mpsc::Sender<()>,
) -> AudioResult<cpal::Stream> {
    let err_fn = move |err: cpal::StreamError| {
        eprintln!("[obsidianrecorder] erreur du flux audio de sortie : {err}");
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                fill_output_buffer(data, &samples, &position, output_channels, &finished_tx);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                fill_output_buffer_i16(data, &samples, &position, output_channels, &finished_tx);
            },
            err_fn,
            None,
        ),
        other => {
            return Err(AudioError::BuildStream(format!(
                "format d'échantillon de sortie non pris en charge : {other:?}"
            )))
        }
    };

    stream.map_err(|e| AudioError::BuildStream(e.to_string()))
}

/// Dépose chaque échantillon mono de `source` sur tous les canaux de
/// sortie (upmix simple), et comble de silence une fois `source` épuisé —
/// en signalant alors la fin de lecture via `finished_tx`.
fn fill_output_buffer(
    data: &mut [f32],
    source: &[f32],
    position: &AtomicUsize,
    channels: u16,
    finished_tx: &mpsc::Sender<()>,
) {
    let channels = channels.max(1) as usize;
    let mut pos = position.load(Ordering::Relaxed);
    for frame in data.chunks_mut(channels) {
        let sample = if pos < source.len() { source[pos] } else { 0.0 };
        for out in frame.iter_mut() {
            *out = sample;
        }
        if pos < source.len() {
            pos += 1;
        }
    }
    position.store(pos, Ordering::Relaxed);
    if pos >= source.len() {
        let _ = finished_tx.send(());
    }
}

/// Variante PCM 16 bits signé de [`fill_output_buffer`], pour les
/// périphériques de sortie qui n'exposent pas le format flottant.
fn fill_output_buffer_i16(
    data: &mut [i16],
    source: &[f32],
    position: &AtomicUsize,
    channels: u16,
    finished_tx: &mpsc::Sender<()>,
) {
    let channels = channels.max(1) as usize;
    let mut pos = position.load(Ordering::Relaxed);
    for frame in data.chunks_mut(channels) {
        let sample_f32 = if pos < source.len() { source[pos] } else { 0.0 };
        let sample_i16 = (sample_f32.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        for out in frame.iter_mut() {
            *out = sample_i16;
        }
        if pos < source.len() {
            pos += 1;
        }
    }
    position.store(pos, Ordering::Relaxed);
    if pos >= source.len() {
        let _ = finished_tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_output_buffer_upmixes_mono_to_stereo() {
        let source = vec![0.5_f32, -0.5, 0.25];
        let position = AtomicUsize::new(0);
        let (tx, _rx) = mpsc::channel();
        let mut data = vec![0.0_f32; 6]; // 3 trames stéréo
        fill_output_buffer(&mut data, &source, &position, 2, &tx);
        assert_eq!(data, vec![0.5, 0.5, -0.5, -0.5, 0.25, 0.25]);
        assert_eq!(position.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn fill_output_buffer_pads_silence_past_end_of_source_and_signals_finished() {
        let source = vec![1.0_f32];
        let position = AtomicUsize::new(0);
        let (tx, rx) = mpsc::channel();
        let mut data = vec![0.0_f32; 4]; // 2 trames stéréo, 1 seul échantillon source
        fill_output_buffer(&mut data, &source, &position, 2, &tx);
        assert_eq!(data, vec![1.0, 1.0, 0.0, 0.0]);
        assert!(rx.try_recv().is_ok(), "la fin de lecture doit être signalée");
    }

    #[test]
    fn fill_output_buffer_i16_quantizes_and_clips() {
        let source = vec![2.0_f32, -2.0];
        let position = AtomicUsize::new(0);
        let (tx, _rx) = mpsc::channel();
        let mut data = vec![0_i16; 2]; // mono
        fill_output_buffer_i16(&mut data, &source, &position, 1, &tx);
        assert_eq!(data, vec![i16::MAX, -i16::MAX]);
    }

    #[test]
    fn playing_twice_without_stopping_is_rejected() {
        // Comme pour `AudioCapture` : on simule un thread déjà actif sans
        // construire de vrai flux (pas de matériel audio garanti en CI).
        let (control_tx, _control_rx) = mpsc::channel::<PlaybackCommand>();
        let worker = thread::spawn(|| -> AudioResult<()> { Ok(()) });
        let mut playback = AudioPlayback {
            control_tx: Some(control_tx),
            worker: Some(worker),
        };

        assert!(playback.is_playing());
        let result = playback.play("quelconque.mp3");
        assert!(matches!(result, Err(AudioError::AlreadyPlaying)));

        let _ = playback.stop();
    }

    #[test]
    fn stopping_when_not_playing_returns_not_playing_error() {
        let mut playback = AudioPlayback::new();
        assert!(matches!(playback.stop(), Err(AudioError::NotPlaying)));
    }

    #[test]
    fn poll_finished_reports_false_when_nothing_is_playing() {
        let mut playback = AudioPlayback::new();
        assert!(!playback.poll_finished());
    }
}
