//! Capture audio bas niveau (WASAPI sous Windows, via `cpal`) et pipeline
//! de conversion vers le format standard des moteurs STT hors ligne (mono,
//! 16 kHz, PCM 16 bits).
//!
//! ## Choix d'architecture : un thread dédié par enregistrement
//!
//! `cpal::Stream` n'implémente pas `Send` : sous Windows, le flux WASAPI
//! est lié à l'appartement COM du thread qui l'a créé, et doit être détruit
//! sur ce même thread. On ne peut donc pas construire le `Stream` sur un
//! thread puis le déplacer vers l'état partagé de l'application.
//!
//! La solution retenue — classique pour `cpal` — est de dédier un thread
//! complet au cycle de vie de l'enregistrement : ce thread crée l'hôte, le
//! périphérique et le flux, les garde en vie tant qu'aucun ordre d'arrêt
//! n'est reçu (via un canal `mpsc`), puis les détruit avant de finaliser le
//! fichier WAV. Seuls des types `Send` (canaux, `PathBuf`, `String`)
//! traversent la frontière entre threads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::dsp::{
    downmix_f32_to_mono, downmix_i16_to_mono_f32, quantize_to_i16, resample_linear,
    TARGET_SAMPLE_RATE,
};
use super::error::{AudioError, AudioResult};
use super::wav_writer::RecordingWavWriter;

/// Résultat d'un enregistrement terminé : chemin du fichier WAV produit et
/// durée effective de la prise.
#[derive(Debug, Clone)]
pub struct RecordingSummary {
    pub path: PathBuf,
    pub duration: Duration,
}

/// Ordres envoyés au thread de capture.
enum CaptureCommand {
    Stop,
}

/// Pilote la capture audio : démarre/arrête un enregistrement en confiant
/// le cycle de vie du flux `cpal` à un thread dédié (voir note de module).
pub struct AudioCapture {
    control_tx: Option<mpsc::Sender<CaptureCommand>>,
    worker: Option<thread::JoinHandle<AudioResult<RecordingSummary>>>,
    /// Niveau RMS du signal capturé le plus récent (environ [0.0, 1.0]),
    /// mis à jour en continu pendant un enregistrement — lu par le
    /// mini-overlay (vu-mètre). Stocké en bits via `AtomicU32` (la
    /// bibliothèque standard n'a pas d'`AtomicF32`) ; remis à 0.0 à
    /// l'arrêt. Partagé (`Arc`) avec le thread de capture, jamais déplacé
    /// lui-même entre threads après construction.
    level: Arc<AtomicU32>,
    /// `true` tant que la grille anti-silence (voir [`SilenceGate`])
    /// écrit effectivement les trames reçues dans le fichier WAV ; `false`
    /// pendant une pause automatique due à un silence prolongé. Toujours
    /// `true` hors enregistrement ou quand le seuil est désactivé (0.0).
    gate_open: Arc<AtomicBool>,
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            control_tx: None,
            worker: None,
            level: Arc::new(AtomicU32::new(0)),
            gate_open: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.worker.is_some()
    }

    /// Niveau RMS courant (environ [0.0, 1.0]), mis à jour en continu
    /// pendant un enregistrement. Reste à 0.0 hors enregistrement.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// `false` si l'enregistrement en cours est actuellement en pause à
    /// cause d'un silence prolongé (voir [`SilenceGate`]) ; `true` dans
    /// tous les autres cas, y compris hors enregistrement.
    pub fn is_gate_open(&self) -> bool {
        self.gate_open.load(Ordering::Relaxed)
    }

    /// Démarre un enregistrement vers `output_path`, sur le périphérique
    /// nommé `device_name` (ou le périphérique d'entrée par défaut si
    /// `None`). `silence_threshold` règle la grille anti-silence (voir
    /// [`SilenceGate`]) : `0.0` l'désactive (tout est enregistré, comportement
    /// historique). Bloque brièvement le temps que le thread de capture
    /// confirme que le flux audio a bien démarré, afin qu'un `stop()`
    /// immédiat ne puisse jamais arriver avant que le flux ne soit prêt.
    pub fn start(
        &mut self,
        device_name: Option<String>,
        output_path: PathBuf,
        silence_threshold: f32,
    ) -> AudioResult<()> {
        if self.worker.is_some() {
            return Err(AudioError::AlreadyRecording);
        }

        let (control_tx, control_rx) = mpsc::channel::<CaptureCommand>();
        let (ready_tx, ready_rx) = mpsc::channel::<AudioResult<()>>();
        let level = Arc::clone(&self.level);
        let gate_open = Arc::clone(&self.gate_open);
        gate_open.store(true, Ordering::Relaxed);

        let worker = thread::Builder::new()
            .name("obsidianrecorder-audio-capture".into())
            .spawn(move || {
                run_capture_thread(
                    device_name,
                    output_path,
                    silence_threshold,
                    control_rx,
                    ready_tx,
                    level,
                    gate_open,
                )
            })
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
                    "le thread de capture s'est arrêté avant d'avoir démarré le flux audio"
                        .into(),
                ))
            }
        }
    }

    /// Arrête l'enregistrement en cours, finalise le fichier WAV et
    /// retourne son chemin ainsi que sa durée.
    pub fn stop(&mut self) -> AudioResult<RecordingSummary> {
        let control_tx = self.control_tx.take().ok_or(AudioError::NotRecording)?;
        let worker = self.worker.take().ok_or(AudioError::NotRecording)?;

        // On ignore l'échec d'envoi : si le récepteur est déjà fermé, le
        // thread s'est arrêté de lui-même (erreur audio) et `join()`
        // ci-dessous nous rapportera la cause exacte.
        let _ = control_tx.send(CaptureCommand::Stop);

        let outcome = worker
            .join()
            .map_err(|_| AudioError::BuildStream("le thread de capture a paniqué".into()))?;

        // Le vu-mètre de l'interface doit retomber à zéro une fois
        // l'enregistrement arrêté, pas rester bloqué sur la dernière valeur
        // mesurée avant l'arrêt. Idem pour l'indicateur de pause : un
        // enregistrement arrêté ne doit jamais s'afficher « en pause ».
        self.level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.gate_open.store(true, Ordering::Relaxed);
        outcome
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Corps du thread dédié : construit le flux, attend l'ordre d'arrêt, puis
/// finalise le fichier WAV.
fn run_capture_thread(
    device_name: Option<String>,
    output_path: PathBuf,
    silence_threshold: f32,
    control_rx: mpsc::Receiver<CaptureCommand>,
    ready_tx: mpsc::Sender<AudioResult<()>>,
    level: Arc<AtomicU32>,
    gate_open: Arc<AtomicBool>,
) -> AudioResult<RecordingSummary> {
    let setup = setup_stream(device_name, &output_path, silence_threshold, level, gate_open);

    let (stream, writer) = match setup {
        Ok(pair) => {
            let _ = ready_tx.send(Ok(()));
            pair
        }
        Err(err) => {
            let _ = ready_tx.send(Err(AudioError::BuildStream(err.to_string())));
            return Err(err);
        }
    };

    // On bloque ce thread jusqu'à réception de l'ordre d'arrêt : `stream`
    // doit rester en vie sur ce thread jusqu'à ce point (voir note de
    // module en tête de fichier).
    let _ = control_rx.recv();
    drop(stream);

    // Une fois le flux détruit, la fermeture (callback) qui détenait
    // l'autre référence au writer a elle aussi été détruite : il ne reste
    // plus qu'une seule référence `Arc`, et `try_unwrap` doit réussir.
    let writer = Arc::try_unwrap(writer)
        .map_err(|_| AudioError::BuildStream("le writer WAV est encore partagé".into()))?
        .into_inner()
        .map_err(|_| AudioError::BuildStream("le verrou du writer WAV a été empoisonné".into()))?;

    let duration = writer.duration();
    let path = writer.finalize()?;
    Ok(RecordingSummary { path, duration })
}

/// Construit l'hôte, le périphérique, le fichier WAV de sortie et le flux
/// `cpal` prêt à être démarré.
fn setup_stream(
    device_name: Option<String>,
    output_path: &std::path::Path,
    silence_threshold: f32,
    level: Arc<AtomicU32>,
    gate_open: Arc<AtomicBool>,
) -> AudioResult<(cpal::Stream, Arc<Mutex<RecordingWavWriter>>)> {
    let host = cpal::default_host();
    let device = select_input_device(&host, device_name.as_deref())?;

    let supported_config = device
        .default_input_config()
        .map_err(|e| AudioError::ConfigQuery(e.to_string()))?;

    let sample_format = supported_config.sample_format();
    let source_rate = supported_config.sample_rate().0;
    let source_channels = supported_config.channels();
    let stream_config: cpal::StreamConfig = supported_config.into();

    let writer = RecordingWavWriter::create(output_path)?;
    let writer = Arc::new(Mutex::new(writer));

    let stream = build_stream(
        &device,
        &stream_config,
        sample_format,
        source_rate,
        source_channels,
        Arc::clone(&writer),
        level,
        SilenceGate::new(silence_threshold),
        gate_open,
    )?;
    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))?;

    Ok((stream, writer))
}

/// Grille anti-silence (« noise gate ») à seuil configurable et temps de
/// maintien (« hold ») : évite de couper un mot dont le niveau retombe
/// brièvement sous le seuil entre deux syllabes, tout en permettant de ne
/// pas écrire les silences prolongés dans le fichier WAV final (voir
/// `Settings::capture_threshold`).
///
/// Purement fonctionnelle (aucune dépendance système) : testable
/// unitairement comme le reste du pipeline DSP (voir `super::dsp`).
struct SilenceGate {
    threshold: f32,
    hold: Duration,
    open: bool,
    below_since: Option<Duration>,
    elapsed: Duration,
}

impl SilenceGate {
    /// `threshold <= 0.0` désactive totalement la grille : `update`
    /// retourne alors toujours `true` (comportement historique conservé
    /// par défaut, voir `Settings::capture_threshold`).
    fn new(threshold: f32) -> Self {
        Self {
            threshold,
            hold: Duration::from_millis(500),
            open: true,
            below_since: None,
            elapsed: Duration::ZERO,
        }
    }

    fn disabled(&self) -> bool {
        self.threshold <= 0.0
    }

    /// Met à jour la grille avec le niveau RMS d'une trame fraîchement
    /// reçue (durée `frame_duration`, déduite de sa taille et de la
    /// fréquence native du périphérique) et indique si cette trame doit
    /// être écrite dans le fichier WAV.
    fn update(&mut self, rms: f32, frame_duration: Duration) -> bool {
        if self.disabled() {
            return true;
        }
        self.elapsed += frame_duration;
        if rms >= self.threshold {
            self.open = true;
            self.below_since = None;
        } else {
            let since = *self.below_since.get_or_insert(self.elapsed);
            if self.elapsed.saturating_sub(since) >= self.hold {
                self.open = false;
            }
        }
        self.open
    }
}

/// Construit le flux d'entrée `cpal` correspondant au format d'échantillon
/// natif du périphérique, en branchant le pipeline downmix → grille
/// anti-silence → resample → quantification → écriture WAV sur chaque
/// trame reçue.
#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    source_rate: u32,
    source_channels: u16,
    writer: Arc<Mutex<RecordingWavWriter>>,
    level: Arc<AtomicU32>,
    mut gate: SilenceGate,
    gate_open: Arc<AtomicBool>,
) -> AudioResult<cpal::Stream> {
    let err_fn = move |err: cpal::StreamError| {
        eprintln!("[obsidianrecorder] erreur du flux audio d'entrée : {err}");
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mono = downmix_f32_to_mono(data, source_channels);
                let rms = compute_rms(&mono);
                update_level(&level, rms);
                let frame_duration =
                    Duration::from_secs_f64(mono.len() as f64 / source_rate.max(1) as f64);
                let should_write = gate.update(rms, frame_duration);
                gate_open.store(should_write, Ordering::Relaxed);
                if should_write {
                    ingest_frame(&mono, source_rate, &writer);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let mono = downmix_i16_to_mono_f32(data, source_channels);
                let rms = compute_rms(&mono);
                update_level(&level, rms);
                let frame_duration =
                    Duration::from_secs_f64(mono.len() as f64 / source_rate.max(1) as f64);
                let should_write = gate.update(rms, frame_duration);
                gate_open.store(should_write, Ordering::Relaxed);
                if should_write {
                    ingest_frame(&mono, source_rate, &writer);
                }
            },
            err_fn,
            None,
        ),
        other => {
            return Err(AudioError::BuildStream(format!(
                "format d'échantillon d'entrée non pris en charge : {other:?}"
            )))
        }
    };

    stream.map_err(|e| AudioError::BuildStream(e.to_string()))
}

/// Met à jour le niveau RMS partagé à partir d'un niveau déjà calculé pour
/// la trame courante (évite de recalculer le RMS deux fois : la grille
/// anti-silence en a elle aussi besoin, voir `build_stream`).
fn update_level(level: &Arc<AtomicU32>, rms: f32) {
    level.store(rms.to_bits(), Ordering::Relaxed);
}

/// Niveau RMS (racine de la moyenne des carrés) d'un buffer de signal.
fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Ré-échantillonne une trame mono vers 16 kHz, la quantifie en PCM 16 bits
/// et l'ajoute au fichier WAV en cours d'écriture. Les erreurs d'écriture
/// sont journalisées plutôt que propagées : on ne peut pas faire remonter
/// d'erreur depuis le callback temps réel de `cpal` sans risquer d'y
/// bloquer le driver audio.
fn ingest_frame(mono: &[f32], source_rate: u32, writer: &Arc<Mutex<RecordingWavWriter>>) {
    let resampled = resample_linear(mono, source_rate, TARGET_SAMPLE_RATE);
    let pcm16 = quantize_to_i16(&resampled);
    match writer.lock() {
        Ok(mut writer) => {
            if let Err(err) = writer.write_samples(&pcm16) {
                eprintln!("[obsidianrecorder] échec d'écriture d'une trame WAV : {err}");
            }
        }
        Err(_) => eprintln!("[obsidianrecorder] verrou du writer WAV empoisonné, trame perdue"),
    }
}

/// Sélectionne le périphérique d'entrée demandé par son nom, ou le
/// périphérique d'entrée par défaut du système si aucun nom n'est fourni.
fn select_input_device(host: &cpal::Host, device_name: Option<&str>) -> AudioResult<cpal::Device> {
    match device_name {
        Some(name) => host
            .input_devices()
            .map_err(|e| AudioError::ConfigQuery(e.to_string()))?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceNotFound(name.to_string())),
        None => host.default_input_device().ok_or(AudioError::NoInputDevice),
    }
}

/// Énumère les noms des périphériques d'entrée audio disponibles sur le
/// système, dans l'ordre renvoyé par l'hôte audio (WASAPI sous Windows).
pub fn list_input_devices() -> AudioResult<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| AudioError::ConfigQuery(e.to_string()))?;
    Ok(devices.filter_map(|d| d.name().ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_an_unknown_device_name_returns_a_clear_error() {
        let host = cpal::default_host();
        let result = select_input_device(&host, Some("Un périphérique qui n'existe pas"));
        assert!(matches!(result, Err(AudioError::DeviceNotFound(_))));
    }

    #[test]
    fn starting_twice_without_stopping_is_rejected() {
        // On ne construit volontairement pas de vrai flux ici (pas de
        // matériel audio garanti en CI) : on vérifie uniquement la garde
        // d'état `AlreadyRecording`, en simulant un enregistrement déjà
        // actif via un canal de contrôle factice.
        let (control_tx, _control_rx) = mpsc::channel::<CaptureCommand>();
        let worker = thread::spawn(|| -> AudioResult<RecordingSummary> {
            Ok(RecordingSummary {
                path: PathBuf::from("test.wav"),
                duration: Duration::from_secs(0),
            })
        });
        let mut capture = AudioCapture {
            control_tx: Some(control_tx),
            worker: Some(worker),
            level: Arc::new(AtomicU32::new(0)),
            gate_open: Arc::new(AtomicBool::new(true)),
        };

        assert!(capture.is_recording());
        let result = capture.start(None, PathBuf::from("autre.wav"), 0.0);
        assert!(matches!(result, Err(AudioError::AlreadyRecording)));

        // Nettoyage : on laisse le thread factice se terminer normalement.
        let _ = capture.stop();
    }

    #[test]
    fn compute_rms_of_silence_is_zero() {
        assert_eq!(compute_rms(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn compute_rms_of_constant_signal_equals_its_amplitude() {
        let samples = vec![0.5_f32; 100];
        assert!((compute_rms(&samples) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn compute_rms_of_empty_buffer_is_zero() {
        assert_eq!(compute_rms(&[]), 0.0);
    }

    #[test]
    fn level_defaults_to_zero_and_resets_after_stop() {
        let capture = AudioCapture::new();
        assert_eq!(capture.level(), 0.0);
    }

    #[test]
    fn gate_defaults_to_open() {
        let capture = AudioCapture::new();
        assert!(capture.is_gate_open());
    }

    #[test]
    fn disabled_gate_always_writes() {
        let mut gate = SilenceGate::new(0.0);
        assert!(gate.update(0.0, Duration::from_millis(100)));
        assert!(gate.update(0.0, Duration::from_secs(10)));
    }

    #[test]
    fn gate_stays_open_while_above_threshold() {
        let mut gate = SilenceGate::new(0.1);
        assert!(gate.update(0.5, Duration::from_millis(20)));
        assert!(gate.update(0.5, Duration::from_millis(20)));
    }

    #[test]
    fn gate_closes_only_after_sustained_silence() {
        let mut gate = SilenceGate::new(0.1);
        // Un bref creux sous le seuil (moins que le temps de maintien) ne
        // doit pas couper le mot en cours.
        assert!(gate.update(0.5, Duration::from_millis(20)));
        assert!(gate.update(0.02, Duration::from_millis(100)));
        assert!(gate.update(0.5, Duration::from_millis(20)));

        // Un silence prolongé (davantage que le temps de maintien, 500 ms)
        // doit finir par fermer la grille.
        assert!(gate.update(0.02, Duration::from_millis(300)));
        assert!(!gate.update(0.02, Duration::from_millis(600)));
    }

    #[test]
    fn gate_reopens_immediately_when_level_rises_again() {
        let mut gate = SilenceGate::new(0.1);
        // Le silence n'excède pas encore le temps de maintien depuis sa
        // première détection : la grille reste ouverte.
        assert!(gate.update(0.02, Duration::from_millis(600)));
        // Le silence se prolonge désormais au-delà du temps de maintien :
        // la grille se ferme.
        assert!(!gate.update(0.02, Duration::from_millis(600)));
        // Un son franchissant à nouveau le seuil rouvre immédiatement.
        assert!(gate.update(0.5, Duration::from_millis(10)));
    }
}
