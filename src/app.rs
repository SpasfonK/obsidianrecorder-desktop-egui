//! Interface graphique (egui/eframe), intégration du raccourci clavier
//! global, de la transcription hors ligne (Whisper), de l'écriture dans le
//! Vault Obsidian, des réglages persistés et de l'icône System Tray.
//!
//! Aucune dépendance à un runtime externe : pas de WebView2, pas de
//! Node/npm, pas d'installateur. whisper.cpp est lié statiquement ; `rfd`
//! et `tray-icon` s'appuient sur les API Windows déjà présentes (COM).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;

use obsidianrecorder_lib::audio::{self, AudioCapture, AudioError, AudioPlayback, AudioResult};
use obsidianrecorder_lib::hotkey::{HotkeyAction, HotkeyBinding, HotkeyError, HotkeyRegistry};
use obsidianrecorder_lib::paths;
use obsidianrecorder_lib::settings::{self, Settings, ThemePreference, WriteModeSetting};
use obsidianrecorder_lib::stt::model::{default_model_is_present, default_model_path};
use obsidianrecorder_lib::stt::Transcriber;
use obsidianrecorder_lib::vault::{write_note, FrontmatterData, NoteData, Vault, WriteMode};

/// Raccourci global par défaut : bascule démarrage/arrêt de l'enregistrement
/// depuis n'importe quelle application.
const DEFAULT_TOGGLE_HOTKEY: &str = "Ctrl+Shift+O";

/// Langue cible de la transcription (le projet est conçu pour le français
/// en priorité, cf. le modèle Android d'origine).
const STT_LANGUAGE: &str = "fr";

/// Sous-dossiers par défaut dans le Vault, créés au besoin.
const AUDIO_NOTES_SUBFOLDER: &str = "AudioNotes";
const DAILY_NOTES_SUBFOLDER: &str = "Daily Notes";
const ATTACHMENTS_SUBFOLDER: &str = "attachments";

type SharedAudio = Arc<Mutex<AudioCapture>>;
type SharedLastRecording = Arc<Mutex<Option<String>>>;
type SharedStt = Arc<Mutex<SttState>>;
type SharedVault = Arc<Mutex<Option<Vault>>>;
type SharedNoteStatus = Arc<Mutex<NoteStatus>>;
type SharedSettings = Arc<Mutex<Settings>>;

/// État du sous-système de transcription, partagé entre le thread de
/// chargement du modèle, le thread d'écoute du raccourci global, le thread
/// de transcription et l'interface.
enum SttState {
    Loading,
    Unavailable(String),
    Ready(Arc<Transcriber>),
    Transcribing,
    Done(String),
    Failed(String),
}

/// État de la dernière tentative d'écriture d'une note dans le Vault.
enum NoteStatus {
    Idle,
    Written(PathBuf),
    Failed(String),
}

/// État partagé entre l'interface, le thread de chargement du modèle, le
/// thread d'écoute du raccourci global et le thread de transcription. Le
/// cloner est peu coûteux : chaque champ est un `Arc`.
#[derive(Clone)]
struct AppContext {
    audio: SharedAudio,
    last_recording: SharedLastRecording,
    stt: SharedStt,
    vault: SharedVault,
    note_status: SharedNoteStatus,
    settings: SharedSettings,
}

pub struct ObsidianRecorderApp {
    ctx: AppContext,
    devices: Vec<String>,
    selected_device: Option<String>,
    last_error: Option<String>,
    hotkey_status: String,
    tray_status: String,
    vault_path_display: Option<String>,
    vault_error: Option<String>,
    show_settings: bool,
    /// Texte brut saisi par l'utilisateur pour les tags supplémentaires —
    /// jamais reconstruit depuis `settings.extra_tags` après le chargement
    /// initial, pour ne pas perturber la saisie en cours (voir
    /// `update()`).
    tags_input: String,
    /// Lecteur audio d'aperçu (dashboard des notes récentes) — propre à
    /// l'interface, aucun autre thread n'y touche.
    playback: AudioPlayback,
}

impl ObsidianRecorderApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let loaded_settings = settings::load();
        let tags_input = loaded_settings.extra_tags.join(", ");

        let ctx = AppContext {
            audio: Arc::new(Mutex::new(AudioCapture::new())),
            last_recording: Arc::new(Mutex::new(None)),
            stt: Arc::new(Mutex::new(SttState::Loading)),
            vault: Arc::new(Mutex::new(None)),
            note_status: Arc::new(Mutex::new(NoteStatus::Idle)),
            settings: Arc::new(Mutex::new(loaded_settings.clone())),
        };

        // Le modèle Whisper (des centaines de Mo) se charge en tâche de
        // fond : la fenêtre s'affiche immédiatement, sans attendre.
        spawn_model_loader(Arc::clone(&ctx.stt));

        let hotkey_status = match setup_global_hotkey(ctx.clone()) {
            Ok(()) => format!("Raccourci actif : {DEFAULT_TOGGLE_HOTKEY}"),
            Err(err) => format!("Raccourci indisponible ({err}) — utilisez le bouton ci-dessous"),
        };

        let tray_status = if setup_tray_icon() {
            "Icône System Tray active (clic droit pour le menu).".to_string()
        } else {
            "Icône System Tray indisponible sur ce système.".to_string()
        };

        let devices = audio::list_input_devices().unwrap_or_default();
        let selected_device = devices.first().cloned();

        // Si un Vault a déjà été choisi lors d'un lancement précédent, on
        // le revalide (il peut avoir été déplacé/supprimé depuis).
        let (vault_path_display, vault_error) = match &loaded_settings.vault_path {
            Some(path) => match Vault::open(path) {
                Ok(vault) => {
                    let display = vault.root().display().to_string();
                    if let Ok(mut guard) = ctx.vault.lock() {
                        *guard = Some(vault);
                    }
                    (Some(display), None)
                }
                Err(err) => (
                    None,
                    Some(format!("Vault précédemment configuré introuvable : {err}")),
                ),
            },
            None => (None, None),
        };

        Self {
            ctx,
            devices,
            selected_device,
            last_error: None,
            hotkey_status,
            tray_status,
            vault_path_display,
            vault_error,
            show_settings: false,
            tags_input,
            playback: AudioPlayback::new(),
        }
    }

    /// Ouvre le sélecteur de dossier natif, valide le Vault choisi et
    /// persiste son chemin dans les réglages.
    fn choose_vault(&mut self) {
        self.vault_error = None;
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Choisir le Vault Obsidian")
            .pick_folder()
        else {
            return; // L'utilisateur a annulé : on ne change rien.
        };

        match Vault::open(&folder) {
            Ok(vault) => {
                self.vault_path_display = Some(vault.root().display().to_string());
                if let Ok(mut guard) = self.ctx.vault.lock() {
                    *guard = Some(vault);
                }
                if let Ok(mut settings) = self.ctx.settings.lock() {
                    settings.vault_path = Some(folder);
                    let _ = settings::save(&settings);
                }
            }
            Err(err) => self.vault_error = Some(err.to_string()),
        }
    }
}

impl eframe::App for ObsidianRecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Le raccourci global et le menu System Tray peuvent démarrer/
        // arrêter l'enregistrement pendant que la fenêtre n'est pas au
        // premier plan : on redessine régulièrement pour rester synchronisé.
        ctx.request_repaint_after(Duration::from_millis(500));

        // Applique la préférence de thème choisie dans les réglages.
        // "Système" laisse le thème par défaut d'eframe inchangé.
        if let Ok(settings) = self.ctx.settings.lock() {
            match settings.theme {
                ThemePreference::Light => ctx.set_visuals(egui::Visuals::light()),
                ThemePreference::Dark => ctx.set_visuals(egui::Visuals::dark()),
                ThemePreference::System => {}
            }
        }

        // Événements du menu contextuel System Tray.
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "toggle_recording" => {
                    let device = self.selected_device.clone();
                    if let Err(err) = toggle_recording(&self.ctx, device) {
                        self.last_error = Some(err.to_string());
                    }
                }
                "open_vault" => {
                    if let Ok(guard) = self.ctx.vault.lock() {
                        if let Some(vault) = guard.as_ref() {
                            let _ = std::process::Command::new("explorer")
                                .arg(vault.root())
                                .spawn();
                        }
                    }
                }
                "preferences" => self.show_settings = true,
                "quit" => {
                    // Sécurité : on finalise un enregistrement en cours
                    // avant de quitter, pour ne jamais perdre un WAV non
                    // finalisé (sans en-tête RIFF correct).
                    let is_recording = self
                        .ctx
                        .audio
                        .lock()
                        .map(|g| g.is_recording())
                        .unwrap_or(false);
                    if is_recording {
                        let _ = toggle_recording(&self.ctx, None);
                    }
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        let recording = self
            .ctx
            .audio
            .lock()
            .map(|guard| guard.is_recording())
            .unwrap_or(false);
        let level = self.ctx.audio.lock().map(|guard| guard.level()).unwrap_or(0.0);
        let error_color = egui::Color32::from_rgb(224, 86, 122);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("ObsidianRECORDER Desktop");
            ui.label("Itération 4 — Audio, Raccourci, STT, Export Obsidian & System Tray");
            ui.separator();

            ui.label("Microphone");
            egui::ComboBox::from_id_source("device-select")
                .selected_text(
                    self.selected_device
                        .clone()
                        .unwrap_or_else(|| "Périphérique par défaut".to_string()),
                )
                .show_ui(ui, |ui| {
                    for device in &self.devices {
                        ui.selectable_value(&mut self.selected_device, Some(device.clone()), device);
                    }
                });

            ui.add_space(12.0);

            ui.horizontal(|ui| {
                let color = if recording { error_color } else { egui::Color32::GRAY };
                ui.colored_label(color, "●");
                ui.label(if recording { "Enregistrement en cours…" } else { "Prêt" });
            });

            ui.add_space(8.0);
            let button_label = if recording {
                "Arrêter l'enregistrement"
            } else {
                "Démarrer l'enregistrement"
            };
            if ui.button(button_label).clicked() {
                self.last_error = None;
                let device = self.selected_device.clone();
                if let Err(err) = toggle_recording(&self.ctx, device) {
                    self.last_error = Some(err.to_string());
                }
            }

            if let Some(err) = &self.last_error {
                ui.add_space(6.0);
                ui.colored_label(error_color, err);
            }

            ui.add_space(12.0);
            ui.label(format!(
                "Raccourci global : {DEFAULT_TOGGLE_HOTKEY} (fonctionne même en arrière-plan)"
            ));
            ui.small(&self.hotkey_status);

            if let Ok(last) = self.ctx.last_recording.lock() {
                if let Some(text) = last.as_ref() {
                    ui.add_space(8.0);
                    ui.label(format!("Dernier enregistrement : {text}"));
                }
            }

            ui.separator();
            ui.label("Transcription (Whisper, hors ligne)");
            match self.ctx.stt.lock() {
                Ok(state) => match &*state {
                    SttState::Loading => {
                        ui.small("Chargement du modèle…");
                    }
                    SttState::Unavailable(msg) => {
                        ui.colored_label(error_color, msg);
                    }
                    SttState::Ready(_) => {
                        ui.small("Modèle prêt.");
                    }
                    SttState::Transcribing => {
                        ui.small("Transcription en cours…");
                    }
                    SttState::Done(text) => {
                        ui.label(text);
                    }
                    SttState::Failed(err) => {
                        ui.colored_label(error_color, err);
                    }
                },
                Err(_) => {
                    ui.small("État de la transcription indisponible.");
                }
            }

            ui.separator();
            ui.label("Vault Obsidian");
            if ui.button("Choisir le Vault Obsidian…").clicked() {
                self.choose_vault();
            }
            match &self.vault_path_display {
                Some(path) => {
                    ui.small(format!("Vault : {path}"));
                }
                None => {
                    ui.small(
                        "Aucun Vault configuré — la transcription reste disponible, mais \
                         aucune note Markdown n'est écrite.",
                    );
                }
            }
            if let Some(err) = &self.vault_error {
                ui.colored_label(error_color, err);
            }
            if let Ok(status) = self.ctx.note_status.lock() {
                match &*status {
                    NoteStatus::Idle => {}
                    NoteStatus::Written(path) => {
                        ui.small(format!("Note écrite : {}", path.display()));
                    }
                    NoteStatus::Failed(err) => {
                        ui.colored_label(error_color, err);
                    }
                }
            }

            ui.separator();
            ui.label("Notes récentes (AudioNotes)");
            // N'inclut jamais les entrées en mode Daily Note : elles vivent
            // dans un fichier partagé par jour, sans .wav associé en 1 pour
            // 1 (voir vault::dashboard).
            let mut play_request: Option<PathBuf> = None;
            let mut stop_request = false;
            if let Ok(vault_guard) = self.ctx.vault.lock() {
                match vault_guard.as_ref() {
                    Some(vault) => {
                        let notes = obsidianrecorder_lib::vault::dashboard::list_recent_notes(
                            &vault.resolve(AUDIO_NOTES_SUBFOLDER),
                            5,
                        );
                        if notes.is_empty() {
                            ui.small("Aucune note pour le moment.");
                        } else {
                            for note in &notes {
                                ui.horizontal(|ui| {
                                    ui.label(format!(
                                        "{} — {}",
                                        note.date_label.as_deref().unwrap_or("date inconnue"),
                                        note.duration_label.as_deref().unwrap_or("durée inconnue"),
                                    ));
                                    let playing = self.playback.is_playing();
                                    let label = if playing { "Pause" } else { "Écouter" };
                                    if ui.button(label).clicked() {
                                        if playing {
                                            stop_request = true;
                                        } else if let Some(stem) =
                                            note.path.file_stem().and_then(|s| s.to_str())
                                        {
                                            play_request = Some(
                                                vault
                                                    .resolve(ATTACHMENTS_SUBFOLDER)
                                                    .join(format!("{stem}.wav")),
                                            );
                                        }
                                    }
                                });
                            }
                        }
                    }
                    None => {
                        ui.small("Configurez un Vault pour voir vos notes récentes ici.");
                    }
                }
            }
            if stop_request {
                let _ = self.playback.stop();
            }
            if let Some(path) = play_request {
                if let Err(err) = self.playback.play(&path) {
                    self.last_error = Some(err.to_string());
                }
            }
            self.playback.poll_finished();

            ui.separator();
            ui.checkbox(&mut self.show_settings, "Réglages");
            if self.show_settings {
                ui.group(|ui| {
                    if let Ok(mut settings) = self.ctx.settings.lock() {
                        ui.label("Mode d'écriture des notes");
                        let mut mode_changed = false;
                        mode_changed |= ui
                            .radio_value(
                                &mut settings.write_mode,
                                WriteModeSetting::NewFile,
                                "Nouvelle note horodatée",
                            )
                            .clicked();
                        mode_changed |= ui
                            .radio_value(
                                &mut settings.write_mode,
                                WriteModeSetting::DailyNote,
                                "Ajouter à la Daily Note du jour",
                            )
                            .clicked();

                        ui.add_space(8.0);
                        ui.label("Thème");
                        let mut theme_changed = false;
                        theme_changed |= ui
                            .radio_value(&mut settings.theme, ThemePreference::System, "Système")
                            .clicked();
                        theme_changed |= ui
                            .radio_value(&mut settings.theme, ThemePreference::Light, "Clair")
                            .clicked();
                        theme_changed |= ui
                            .radio_value(&mut settings.theme, ThemePreference::Dark, "Sombre")
                            .clicked();

                        if mode_changed || theme_changed {
                            let _ = settings::save(&settings);
                        }
                    }
                });

                ui.add_space(8.0);
                ui.label("Tags supplémentaires (séparés par des virgules)");
                if ui.text_edit_singleline(&mut self.tags_input).changed() {
                    if let Ok(mut settings) = self.ctx.settings.lock() {
                        settings.extra_tags = self
                            .tags_input
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        let _ = settings::save(&settings);
                    }
                }
            }

            ui.add_space(6.0);
            ui.small(&self.tray_status);
        });

        // Mini-overlay flottant : visible uniquement pendant un
        // enregistrement actif. Une « immediate viewport » egui ne reste
        // affichée que tant qu'on continue à l'appeler à chaque frame — dès
        // que `recording` redevient `false`, on cesse de l'appeler et elle
        // disparaît d'elle-même, sans état supplémentaire à gérer.
        //
        // Volontairement sans `with_transparent`/`with_fullscreen` : leur
        // combinaison avec `with_always_on_top` est connue pour planter
        // egui (voir README). Un simple facteur d'amplification rend la
        // barre de niveau lisible (le RMS brut d'une voix normale reste
        // visuellement très bas sur une échelle [0.0, 1.0]).
        if recording {
            let display_level = (level * 4.0).clamp(0.0, 1.0);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("obsidianrecorder-level-overlay"),
                egui::ViewportBuilder::default()
                    .with_title("ObsidianRECORDER")
                    .with_inner_size([220.0, 70.0])
                    .with_always_on_top()
                    .with_decorations(false)
                    .with_resizable(false),
                |overlay_ctx, class| {
                    if class != egui::ViewportClass::Immediate {
                        return;
                    }
                    egui::CentralPanel::default().show(overlay_ctx, |ui| {
                        ui.label("● Enregistrement en cours");
                        ui.add(egui::ProgressBar::new(display_level));
                    });
                },
            );
        }
    }
}

fn default_output_path() -> std::io::Result<PathBuf> {
    let dir = paths::fallback_recordings_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(paths::timestamped_wav_filename()))
}

/// Bascule l'enregistrement (démarre si arrêté, arrête et finalise sinon).
/// Déclenche automatiquement une transcription en tâche de fond dès que
/// l'arrêt produit un fichier WAV.
///
/// Partagée entre le clic du bouton, le menu System Tray et le raccourci
/// global (threads différents, voir [`setup_global_hotkey`]).
fn toggle_recording(ctx: &AppContext, device: Option<String>) -> AudioResult<()> {
    let mut capture = ctx
        .audio
        .lock()
        .map_err(|_| AudioError::BuildStream("état audio verrouillé de façon incohérente".into()))?;

    if capture.is_recording() {
        let summary = capture.stop()?;
        let text = format!(
            "{} ({:.1} s)",
            summary.path.display(),
            summary.duration.as_secs_f64()
        );
        if let Ok(mut last) = ctx.last_recording.lock() {
            *last = Some(text);
        }
        drop(capture);
        spawn_transcription(ctx.clone(), summary.path, summary.duration);
        return Ok(());
    }

    let output_path = default_output_path().map_err(|e| AudioError::BuildStream(e.to_string()))?;
    capture.start(device, output_path)
}

/// Charge le modèle Whisper en tâche de fond au démarrage de l'application.
fn spawn_model_loader(stt: SharedStt) {
    std::thread::spawn(move || {
        let model_path = default_model_path();
        if !default_model_is_present() {
            if let Ok(mut guard) = stt.lock() {
                *guard = SttState::Unavailable(format!(
                    "Modèle Whisper introuvable ({}). Lancez scripts/download-model.ps1.",
                    model_path.display()
                ));
            }
            return;
        }

        let outcome = Transcriber::load(&model_path, STT_LANGUAGE);
        if let Ok(mut guard) = stt.lock() {
            *guard = match outcome {
                Ok(transcriber) => SttState::Ready(Arc::new(transcriber)),
                Err(err) => SttState::Unavailable(err.to_string()),
            };
        }
    });
}

/// Transcrit un enregistrement fraîchement terminé puis, si un Vault est
/// configuré, écrit automatiquement la note Markdown correspondante selon
/// le mode et les tags actuellement réglés. Tout se passe en tâche de
/// fond, sans bloquer l'interface.
fn spawn_transcription(ctx: AppContext, wav_path: PathBuf, duration: Duration) {
    std::thread::spawn(move || {
        let transcriber = {
            let guard = match ctx.stt.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match &*guard {
                SttState::Ready(t) => Arc::clone(t),
                // Modèle pas encore chargé, ou indisponible : on abandonne
                // silencieusement plutôt que de bloquer l'enregistrement.
                _ => return,
            }
        };

        if let Ok(mut guard) = ctx.stt.lock() {
            *guard = SttState::Transcribing;
        }

        let result = transcriber.transcribe_wav_file(&wav_path);
        let transcript_text_for_vault = result.as_ref().ok().map(|t| t.full_text());

        if let Ok(mut guard) = ctx.stt.lock() {
            *guard = match result {
                Ok(transcript) => SttState::Done(transcript.full_text()),
                Err(err) => SttState::Failed(err.to_string()),
            };
        }

        let Some(transcript_text) = transcript_text_for_vault else {
            return;
        };

        let vault = ctx.vault.lock().ok().and_then(|guard| guard.clone());
        let Some(vault) = vault else {
            return; // Pas de Vault configuré : rien de plus à faire.
        };

        let (write_mode_setting, extra_tags) = ctx
            .settings
            .lock()
            .map(|s| (s.write_mode, s.extra_tags.clone()))
            .unwrap_or_default();

        let outcome = write_note_to_vault(
            &vault,
            &wav_path,
            &transcript_text,
            duration,
            write_mode_setting,
            extra_tags,
        );
        if let Ok(mut guard) = ctx.note_status.lock() {
            *guard = match outcome {
                Ok(note_path) => NoteStatus::Written(note_path),
                Err(err) => NoteStatus::Failed(err),
            };
        }
    });
}

/// Copie l'enregistrement dans `attachments/`, construit la note Markdown
/// (frontmatter + transcription) et l'écrit selon le mode choisi dans les
/// réglages (nouvelle note dans `AudioNotes/`, ou ajout à la Daily Note du
/// jour dans `Daily Notes/`).
fn write_note_to_vault(
    vault: &Vault,
    wav_path: &Path,
    transcript_text: &str,
    duration: Duration,
    write_mode_setting: WriteModeSetting,
    extra_tags: Vec<String>,
) -> Result<PathBuf, String> {
    let attachments_dir = vault.resolve(ATTACHMENTS_SUBFOLDER);
    std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;

    let wav_filename = wav_path
        .file_name()
        .ok_or_else(|| "nom de fichier audio invalide".to_string())?;
    let attachment_dest = attachments_dir.join(wav_filename);
    std::fs::copy(wav_path, &attachment_dest).map_err(|e| e.to_string())?;

    let audio_relative = format!("{ATTACHMENTS_SUBFOLDER}/{}", wav_filename.to_string_lossy());

    let note_data = NoteData {
        frontmatter: FrontmatterData {
            date: chrono::Local::now(),
            duration,
            extra_tags,
            audio_file: Some(audio_relative),
        },
        transcript_text: transcript_text.to_string(),
    };

    let content = obsidianrecorder_lib::vault::note::render_note(&note_data);

    // Le nom de la note reprend l'horodatage de l'enregistrement d'origine
    // (même racine que le .wav), pour que les deux restent faciles à
    // rapprocher même en dehors du frontmatter. Ignoré en mode Daily Note
    // (voir `vault::writer::WriteMode`).
    let stem = wav_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("note");
    let filename = format!("{stem}.md");

    let mode = match write_mode_setting {
        WriteModeSetting::NewFile => WriteMode::NewFile {
            target_dir: vault.resolve(AUDIO_NOTES_SUBFOLDER),
        },
        WriteModeSetting::DailyNote => WriteMode::AppendToDailyNote {
            daily_notes_dir: vault.resolve(DAILY_NOTES_SUBFOLDER),
        },
    };

    write_note(&mode, &filename, &content).map_err(|e| e.to_string())
}

/// Initialise le raccourci global par défaut et le thread qui écoute ses
/// événements.
///
/// Doit être appelé sur le thread principal : c'est celui qui fait tourner
/// la boucle d'événements Win32 de `winit` (utilisée par `eframe`), requise
/// par `global-hotkey` pour recevoir les événements `WM_HOTKEY`.
fn setup_global_hotkey(ctx: AppContext) -> Result<(), HotkeyError> {
    let binding = HotkeyBinding::parse(DEFAULT_TOGGLE_HOTKEY)
        .expect("le raccourci par défaut doit être une expression valide");

    // `OsHotkeyBackend` (donc `GlobalHotKeyManager`) embarque un handle natif
    // (HWND côté Windows) qui n'implémente pas `Send` : il doit rester sur ce
    // thread principal pendant toute la durée de vie de l'application.
    // `Box::leak` lui donne simplement une durée de vie `'static`.
    let backend: &'static mut obsidianrecorder_lib::hotkey::OsHotkeyBackend =
        Box::leak(Box::new(obsidianrecorder_lib::hotkey::OsHotkeyBackend::new()?));
    backend.register(&binding)?;

    // Registre pur, local à cette fonction : il n'a besoin de vivre que le
    // temps de vérifier l'absence de collision avant d'armer le raccourci.
    let mut registry = HotkeyRegistry::new();
    registry.register(binding, HotkeyAction::ToggleRecording)?;

    std::thread::Builder::new()
        .name("obsidianrecorder-hotkey-listener".into())
        .spawn(move || loop {
            match global_hotkey::GlobalHotKeyEvent::receiver().recv() {
                Ok(event) if event.state == global_hotkey::HotKeyState::Released => {
                    if let Err(err) = toggle_recording(&ctx, None) {
                        eprintln!(
                            "[obsidianrecorder] échec de la bascule d'enregistrement (raccourci) : {err}"
                        );
                    }
                }
                Ok(_) => {}
                // Le canal ne se ferme que si le processus se termine : on
                // arrête simplement la boucle d'écoute.
                Err(_) => break,
            }
        })
        .expect("le thread d'écoute des raccourcis doit pouvoir démarrer");

    Ok(())
}

/// Charge l'icône embarquée (même PNG que la fenêtre principale, voir
/// `main.rs::load_icon`) au format attendu par `tray-icon`.
fn load_tray_icon() -> Option<tray_icon::Icon> {
    let bytes = include_bytes!("../icons/icon.png");
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    tray_icon::Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// Crée l'icône System Tray et son menu contextuel (Démarrer/Arrêter,
/// Ouvrir le Vault, Préférences, Quitter), et les laisse vivre pour toute
/// la durée du processus via `Box::leak` (les supprimer retirerait l'icône
/// de la zone de notification) — même logique que pour
/// `OsHotkeyBackend::new` ci-dessus. Doit être appelée sur le thread
/// principal.
fn setup_tray_icon() -> bool {
    let Some(icon) = load_tray_icon() else {
        return false;
    };

    let menu = tray_icon::menu::Menu::new();
    let toggle = tray_icon::menu::MenuItem::with_id(
        "toggle_recording",
        "Démarrer / arrêter l'enregistrement",
        true,
        None,
    );
    let open_vault =
        tray_icon::menu::MenuItem::with_id("open_vault", "Ouvrir le Vault", true, None);
    let preferences =
        tray_icon::menu::MenuItem::with_id("preferences", "Préférences", true, None);
    let quit = tray_icon::menu::MenuItem::with_id("quit", "Quitter", true, None);

    let appended = menu.append(&toggle).is_ok()
        && menu.append(&open_vault).is_ok()
        && menu.append(&preferences).is_ok()
        && menu
            .append(&tray_icon::menu::PredefinedMenuItem::separator())
            .is_ok()
        && menu.append(&quit).is_ok();
    if !appended {
        return false;
    }

    let Ok(tray) = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("ObsidianRECORDER Desktop")
        .build()
    else {
        return false;
    };

    Box::leak(Box::new(tray));
    true
}
