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

use obsidianrecorder_lib::audio::{
    self, AudioCapture, AudioError, AudioPlayback, AudioResult, CaptureConfig, RecordingFormat,
    RecordingSummary,
};
use obsidianrecorder_lib::hotkey::{HotkeyAction, HotkeyBinding, HotkeyError, HotkeyRegistry};
use obsidianrecorder_lib::paths;
use obsidianrecorder_lib::settings::{
    self, OutputFormatSetting, Settings, ThemePreference, WriteModeSetting,
};
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
/// Erreur survenue en tâche de fond (raccourci clavier global ou menu
/// System Tray), donc hors de tout clic utilisateur direct : ces threads
/// n'ont pas accès à l'état de l'interface, mais doivent tout de même
/// pouvoir signaler un échec plutôt que de le journaliser silencieusement
/// vers une console invisible (voir `toggle_recording`). Relue et
/// affichée par `update()` au prochain repaint.
type SharedBackgroundError = Arc<Mutex<Option<String>>>;

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
    background_error: SharedBackgroundError,
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
    /// Dernière préférence de thème effectivement appliquée et palette
    /// correspondante : évite de reconstruire tout le `Style` egui à chaque
    /// image (voir `update`). `None` tant que le premier style n'a pas été
    /// posé.
    applied_theme: Option<ThemePreference>,
    palette: Palette,
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
            background_error: Arc::new(Mutex::new(None)),
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
        // Préfère le périphérique mémorisé lors d'un lancement précédent
        // s'il est toujours présent ; sinon, retombe sur le premier
        // disponible. Ce même réglage est relu directement depuis
        // `ctx.settings` par `toggle_recording`, y compris depuis le
        // thread du raccourci clavier global et le menu System Tray, pour
        // qu'ils utilisent toujours le même microphone que celui affiché
        // ici (voir le correctif documenté dans le README).
        let selected_device = loaded_settings
            .input_device
            .clone()
            .filter(|d| devices.contains(d))
            .or_else(|| devices.first().cloned());

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
            applied_theme: None,
            palette: palette(true),
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

    /// Ouvre le sélecteur de fichier natif, décode le fichier choisi
    /// (`.wav`, `.mp3` ou `.m4a`), le ré-encode dans le format de sortie
    /// configuré (MP3 par défaut, avec la même découpe qu'un enregistrement
    /// live) puis le transcrit et écrit la note — exactement comme pour un
    /// enregistrement fraîchement terminé (voir
    /// [`spawn_transcription_and_note`]).
    fn import_audio_file(&mut self) {
        self.last_error = None;
        let Some(source) = rfd::FileDialog::new()
            .set_title("Importer un fichier audio")
            .add_filter("Audio", audio::SUPPORTED_IMPORT_EXTENSIONS)
            .pick_file()
        else {
            return; // L'utilisateur a annulé : on ne change rien.
        };

        let dir = match default_recordings_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.last_error = Some(err.to_string());
                return;
            }
        };

        let (format, max_bytes) = self.output_settings();

        match audio::transcode_to_format(
            &source,
            &dir,
            &paths::timestamped_stem(),
            format,
            max_bytes,
        ) {
            Ok(summary) => {
                if let Ok(mut last) = self.ctx.last_recording.lock() {
                    *last = Some(format!(
                        "{} ({:.1} s, importé depuis {})",
                        join_paths(&summary.paths),
                        summary.duration.as_secs_f64(),
                        source.display()
                    ));
                }
                spawn_transcription_and_note(self.ctx.clone(), summary);
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }
    }

    /// Lit le format de sortie et la taille de découpe actuellement réglés.
    fn output_settings(&self) -> (RecordingFormat, Option<u64>) {
        self.ctx
            .settings
            .lock()
            .map(|s| {
                (
                    format_from_setting(s.output_format),
                    audio::chunk_bytes_from_mb(s.chunk_size_mb),
                )
            })
            .unwrap_or((RecordingFormat::Mp3, None))
    }

    /// Contrôles du format de sortie (§2) et de la taille de découpe (§5),
    /// affichés dans la carte « Enregistrement ».
    fn output_controls(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Ok(mut settings) = self.ctx.settings.lock() else {
            return;
        };

        ui.label(
            egui::RichText::new("Format de sortie")
                .size(12.0)
                .color(p.muted),
        );
        let mut changed = false;
        changed |= ui
            .radio_value(
                &mut settings.output_format,
                OutputFormatSetting::Mp3,
                "MP3 — voix IA, ultra léger (~14 Mo/h)",
            )
            .clicked();
        changed |= ui
            .radio_value(
                &mut settings.output_format,
                OutputFormatSetting::Wav,
                "WAV — PCM 16 bits non compressé",
            )
            .clicked();

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Découpe automatique (taille max. par fichier)")
                .size(12.0)
                .color(p.muted),
        );
        ui.horizontal(|ui| {
            for mb in [10_u32, 25, 50, 0] {
                let label = if mb == 0 {
                    "Sans découpe".to_string()
                } else {
                    format!("{mb} Mo")
                };
                if ui
                    .selectable_label(settings.chunk_size_mb == mb, label)
                    .clicked()
                {
                    settings.chunk_size_mb = mb;
                    changed = true;
                }
            }
        });
        ui.label(
            egui::RichText::new(
                "25 Mo correspond à la limite d'upload de l'API Whisper d'OpenAI. Au-delà de la \
                 taille choisie (moins 64 Ko de marge), un nouveau fichier `…_part2.mp3` est \
                 ouvert sans perdre une seule trame.",
            )
            .size(11.0)
            .color(p.muted),
        );

        if changed {
            let _ = settings::save(&settings);
        }
    }
}

/// Traduit le réglage persisté de format vers le format d'encodage réel.
fn format_from_setting(setting: OutputFormatSetting) -> RecordingFormat {
    match setting {
        OutputFormatSetting::Mp3 => RecordingFormat::Mp3,
        OutputFormatSetting::Wav => RecordingFormat::Wav,
    }
}

/// Concatène des chemins pour un affichage compact (un seul chemin s'il n'y
/// en a qu'un, sinon « a, b, … (N morceaux) »).
fn join_paths(paths: &[std::path::PathBuf]) -> String {
    match paths {
        [] => "(aucun fichier)".to_string(),
        [only] => only.display().to_string(),
        many => format!(
            "{} (+{} autre(s))",
            many[0].display(),
            many.len() - 1
        ),
    }
}

/// Palette de couleurs sobre et unifiée, dérivée du mode clair/sombre.
#[derive(Clone, Copy)]
struct Palette {
    bg: egui::Color32,
    card: egui::Color32,
    card_alt: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
    muted: egui::Color32,
    accent: egui::Color32,
    accent_soft: egui::Color32,
    danger: egui::Color32,
    ok: egui::Color32,
}

/// Construit la palette correspondant au mode sombre ou clair. Teintes
/// désaturées (gris bleutés) et un seul accent indigo, pour un rendu sobre.
fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: egui::Color32::from_rgb(0x10, 0x12, 0x16),
            card: egui::Color32::from_rgb(0x18, 0x1B, 0x21),
            card_alt: egui::Color32::from_rgb(0x1F, 0x23, 0x2B),
            border: egui::Color32::from_rgb(0x26, 0x2B, 0x34),
            text: egui::Color32::from_rgb(0xE7, 0xE9, 0xEE),
            muted: egui::Color32::from_rgb(0x8B, 0x93, 0xA1),
            accent: egui::Color32::from_rgb(0x7C, 0x8C, 0xF8),
            accent_soft: egui::Color32::from_rgba_unmultiplied(0x7C, 0x8C, 0xF8, 0x26),
            danger: egui::Color32::from_rgb(0xE5, 0x48, 0x4D),
            ok: egui::Color32::from_rgb(0x46, 0xA7, 0x58),
        }
    } else {
        Palette {
            bg: egui::Color32::from_rgb(0xF4, 0xF5, 0xF7),
            card: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
            card_alt: egui::Color32::from_rgb(0xF0, 0xF1, 0xF4),
            border: egui::Color32::from_rgb(0xE2, 0xE4, 0xE9),
            text: egui::Color32::from_rgb(0x1A, 0x1C, 0x21),
            muted: egui::Color32::from_rgb(0x6A, 0x72, 0x80),
            accent: egui::Color32::from_rgb(0x4F, 0x63, 0xE6),
            accent_soft: egui::Color32::from_rgba_unmultiplied(0x4F, 0x63, 0xE6, 0x1F),
            danger: egui::Color32::from_rgb(0xDC, 0x2B, 0x31),
            ok: egui::Color32::from_rgb(0x2F, 0x9E, 0x44),
        }
    }
}

/// Applique le style personnalisé (couleurs, arrondis, typographie) au
/// contexte egui et renvoie la palette à réutiliser dans l'interface.
fn apply_style(ctx: &egui::Context, pref: ThemePreference) -> Palette {
    let dark = match pref {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::System => ctx.style().visuals.dark_mode,
    };
    let p = palette(dark);

    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = dark;
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.faint_bg_color = p.card_alt;
    v.extreme_bg_color = if dark {
        egui::Color32::from_rgb(0x0B, 0x0D, 0x11)
    } else {
        egui::Color32::WHITE
    };
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.window_rounding = egui::Rounding::same(14.0);
    v.menu_rounding = egui::Rounding::same(10.0);
    v.selection.bg_fill = p.accent_soft.gamma_multiply(2.0);
    v.selection.stroke = egui::Stroke::new(1.0, p.accent);
    v.widgets.noninteractive =
        widget_visuals(p.card, egui::Stroke::new(1.0, p.border), p.text);
    v.widgets.inactive = widget_visuals(p.card_alt, egui::Stroke::new(1.0, p.border), p.text);
    v.widgets.hovered = widget_visuals(
        p.card_alt,
        egui::Stroke::new(1.0, p.accent.gamma_multiply(0.7)),
        p.text,
    );
    v.widgets.active =
        widget_visuals(p.accent, egui::Stroke::new(1.0, p.accent), egui::Color32::WHITE);
    v.widgets.open = widget_visuals(p.card, egui::Stroke::new(1.0, p.border), p.text);

    let mut style = (*ctx.style()).clone();
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    style.spacing.menu_margin = egui::Margin::same(8.0);
    style.spacing.interact_size.y = 30.0;
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(21.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
        ),
    ]
    .into();

    ctx.set_style(style);
    p
}

/// Jeu de couleurs d'un widget egui, à coins arrondis et filet discret.
fn widget_visuals(
    bg: egui::Color32,
    stroke: egui::Stroke,
    fg: egui::Color32,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: stroke,
        rounding: egui::Rounding::same(10.0),
        fg_stroke: egui::Stroke::new(1.0, fg),
        expansion: 0.0,
    }
}

/// Conteneur « carte » : fond contrasté, coins arrondis, filet discret.
fn card_frame(p: &Palette) -> egui::Frame {
    egui::Frame::none()
        .fill(p.card)
        .rounding(egui::Rounding::same(14.0))
        .stroke(egui::Stroke::new(1.0, p.border))
        .inner_margin(egui::Margin::same(14.0))
}

/// Titre de section : petites capitales discrètes.
fn section_title(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .strong()
            .color(p.muted),
    );
    ui.add_space(6.0);
}

/// Vu-mètre linéaire fin, dessiné directement au peintre (aucun widget
/// détourné) : piste `card_alt`, remplissage accent proportionnel au niveau.
/// `threshold`, s'il est fourni et strictement positif, dessine un repère
/// fin à la position du seuil de captation configuré (même échelle
/// d'affichage que `value`), pour aider à le régler en observant le
/// niveau capté en direct.
fn meter(ui: &mut egui::Ui, p: &Palette, value: f32, threshold: Option<f32>) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(4.0), p.card_alt);
    let filled_width = rect.width() * value.clamp(0.0, 1.0);
    if filled_width > 0.5 {
        let filled = egui::Rect::from_min_size(rect.min, egui::vec2(filled_width, rect.height()));
        painter.rect_filled(filled, egui::Rounding::same(4.0), p.accent);
    }
    if let Some(t) = threshold {
        if t > 0.0 {
            let x = rect.min.x + rect.width() * t.clamp(0.0, 1.0);
            painter.line_segment(
                [egui::pos2(x, rect.min.y - 1.0), egui::pos2(x, rect.max.y + 1.0)],
                egui::Stroke::new(1.5, p.danger),
            );
        }
    }
}

impl eframe::App for ObsidianRecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Le raccourci global et le menu System Tray peuvent démarrer/
        // arrêter l'enregistrement pendant que la fenêtre n'est pas au
        // premier plan : on redessine régulièrement pour rester synchronisé.
        ctx.request_repaint_after(Duration::from_millis(500));

        // Applique le thème sobre personnalisé (voir `apply_style`) et
        // récupère la palette de couleurs partagée par toute l'interface.
        // Le style n'est reconstruit que lorsque la préférence de thème
        // change réellement, pas à chaque image.
        let theme_pref = self
            .ctx
            .settings
            .lock()
            .map(|s| s.theme)
            .unwrap_or_default();
        if self.applied_theme != Some(theme_pref) {
            self.palette = apply_style(ctx, theme_pref);
            self.applied_theme = Some(theme_pref);
        }
        let p = self.palette;

        // Événements du menu contextuel System Tray.
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "toggle_recording" => {
                    if let Err(err) = toggle_recording(&self.ctx) {
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
                        let _ = toggle_recording(&self.ctx);
                    }
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        // Relaie vers l'affichage une éventuelle erreur survenue en tâche
        // de fond (raccourci clavier global ou menu System Tray), hors de
        // tout clic direct dans cette fenêtre (voir `SharedBackgroundError`
        // et le thread du raccourci dans `setup_global_hotkey`).
        if let Ok(mut shared) = self.ctx.background_error.lock() {
            if let Some(message) = shared.take() {
                self.last_error = Some(message);
            }
        }

        let recording = self
            .ctx
            .audio
            .lock()
            .map(|guard| guard.is_recording())
            .unwrap_or(false);
        let level = self.ctx.audio.lock().map(|guard| guard.level()).unwrap_or(0.0);
        let display_level = (level * 4.0).clamp(0.0, 1.0);
        let gate_open = self
            .ctx
            .audio
            .lock()
            .map(|guard| guard.is_gate_open())
            .unwrap_or(true);
        let capture_threshold = self
            .ctx
            .settings
            .lock()
            .map(|s| s.capture_threshold)
            .unwrap_or(0.0);
        let threshold_marker = (capture_threshold * 4.0).clamp(0.0, 1.0);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::same(18.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        // En-tête : titre, sous-titre et filet d'accent.
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Obsidian")
                                    .size(23.0)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new("RECORDER")
                                    .size(23.0)
                                    .strong()
                                    .color(p.accent),
                            );
                        });
                        ui.label(
                            egui::RichText::new(
                                "Mémos vocaux locaux · MP3 voix 16 kHz · Whisper hors ligne · \
                                 export Obsidian",
                            )
                            .size(12.0)
                            .color(p.muted),
                        );
                        ui.add_space(10.0);
                        let header_line = ui.available_rect_before_wrap();
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                header_line.min,
                                egui::vec2(header_line.width(), 2.0),
                            ),
                            egui::Rounding::same(1.0),
                            p.accent,
                        );
                        ui.add_space(14.0);

                        // Carte « Enregistrement ».
                        card_frame(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            section_title(ui, &p, "Enregistrement");

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Microphone").size(12.0).color(p.muted),
                                );
                                let previous_device = self.selected_device.clone();
                                egui::ComboBox::from_id_source("device-select")
                                    .selected_text(
                                        self.selected_device.clone().unwrap_or_else(|| {
                                            "Périphérique par défaut".to_string()
                                        }),
                                    )
                                    .width(ui.available_width().max(140.0))
                                    .show_ui(ui, |ui| {
                                        for device in &self.devices {
                                            ui.selectable_value(
                                                &mut self.selected_device,
                                                Some(device.clone()),
                                                device,
                                            );
                                        }
                                    });
                                // Persiste immédiatement le choix : le
                                // raccourci clavier global et le menu
                                // System Tray relisent `ctx.settings`
                                // (pas l'état de cette fenêtre, voir
                                // `toggle_recording`) et doivent voir le
                                // même microphone que celui affiché ici.
                                if self.selected_device != previous_device {
                                    if let Ok(mut settings) = self.ctx.settings.lock() {
                                        settings.input_device = self.selected_device.clone();
                                        let _ = settings::save(&settings);
                                    }
                                }
                            });

                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                let (dot, label) = if recording && !gate_open {
                                    (p.muted, "En pause (silence détecté)…")
                                } else if recording {
                                    (p.danger, "Enregistrement en cours…")
                                } else {
                                    (p.ok, "Prêt")
                                };
                                ui.label(egui::RichText::new("●").size(13.0).color(dot));
                                ui.label(egui::RichText::new(label).color(p.text));
                            });

                            if recording {
                                ui.add_space(8.0);
                                meter(ui, &p, display_level, Some(threshold_marker));
                            }

                            ui.add_space(12.0);
                            let (fill, label) = if recording {
                                (p.danger, "Arrêter l'enregistrement")
                            } else {
                                (p.accent, "Démarrer l'enregistrement")
                            };
                            let record_button = egui::Button::new(
                                egui::RichText::new(label)
                                    .size(15.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(fill)
                            .rounding(egui::Rounding::same(10.0));
                            if ui
                                .add_sized([ui.available_width(), 42.0], record_button)
                                .clicked()
                            {
                                self.last_error = None;
                                if let Err(err) = toggle_recording(&self.ctx) {
                                    self.last_error = Some(err.to_string());
                                }
                            }

                            if let Some(err) = &self.last_error {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new(err.as_str()).color(p.danger));
                            }

                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Raccourci global : {DEFAULT_TOGGLE_HOTKEY} · actif en arrière-plan"
                                ))
                                .size(11.5)
                                .color(p.muted),
                            );
                            ui.label(
                                egui::RichText::new(self.hotkey_status.as_str())
                                    .size(11.0)
                                    .color(p.muted),
                            );

                            if let Ok(last) = self.ctx.last_recording.lock() {
                                if let Some(text) = last.as_ref() {
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Dernier enregistrement : {text}"
                                        ))
                                        .size(11.5)
                                        .color(p.muted),
                                    );
                                }
                            }

                            ui.add_space(12.0);
                            self.output_controls(ui, &p);
                        });

                        ui.add_space(12.0);

                        // Carte « Transcription ».
                        card_frame(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            section_title(ui, &p, "Transcription · Whisper hors ligne");
                            match self.ctx.stt.lock() {
                                Ok(state) => match &*state {
                                    SttState::Loading => {
                                        ui.label(
                                            egui::RichText::new("Chargement du modèle…")
                                                .color(p.muted),
                                        );
                                    }
                                    SttState::Unavailable(msg) => {
                                        ui.label(egui::RichText::new(msg.as_str()).color(p.danger));
                                    }
                                    SttState::Ready(_) => {
                                        ui.label(egui::RichText::new("Modèle prêt.").color(p.ok));
                                    }
                                    SttState::Transcribing => {
                                        ui.label(
                                            egui::RichText::new("Transcription en cours…")
                                                .color(p.muted),
                                        );
                                    }
                                    SttState::Done(text) => {
                                        ui.label(egui::RichText::new(text.as_str()).color(p.text));
                                    }
                                    SttState::Failed(err) => {
                                        ui.label(egui::RichText::new(err.as_str()).color(p.danger));
                                    }
                                },
                                Err(_) => {
                                    ui.label(
                                        egui::RichText::new(
                                            "État de la transcription indisponible.",
                                        )
                                        .color(p.muted),
                                    );
                                }
                            }

                            ui.add_space(10.0);
                            let import_button = egui::Button::new(
                                egui::RichText::new(
                                    "Importer et convertir un fichier (.wav / .mp3 / .m4a)…",
                                )
                                .color(p.text),
                            )
                            .fill(p.card_alt)
                            .stroke(egui::Stroke::new(1.0, p.border))
                            .rounding(egui::Rounding::same(10.0));
                            if ui
                                .add_sized([ui.available_width(), 32.0], import_button)
                                .clicked()
                            {
                                self.import_audio_file();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "Conversion d'un fichier existant vers le format de sortie \
                                     réglé (avec découpe), puis transcription hors ligne — sans \
                                     passer par le microphone.",
                                )
                                .size(11.0)
                                .color(p.muted),
                            );
                        });

                        ui.add_space(12.0);

                        // Carte « Vault Obsidian ».
                        card_frame(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            section_title(ui, &p, "Vault Obsidian");

                            let choose = egui::Button::new(
                                egui::RichText::new("Choisir le Vault Obsidian…").color(p.text),
                            )
                            .fill(p.card_alt)
                            .stroke(egui::Stroke::new(1.0, p.border))
                            .rounding(egui::Rounding::same(10.0));
                            if ui
                                .add_sized([ui.available_width(), 34.0], choose)
                                .clicked()
                            {
                                self.choose_vault();
                            }

                            ui.add_space(8.0);
                            match &self.vault_path_display {
                                Some(path) => {
                                    ui.label(
                                        egui::RichText::new(format!("Vault : {path}"))
                                            .size(11.5)
                                            .color(p.muted),
                                    );
                                }
                                None => {
                                    ui.label(
                                        egui::RichText::new(
                                            "Aucun Vault configuré — la transcription reste \
                                             disponible, mais aucune note Markdown n'est écrite.",
                                        )
                                        .size(11.5)
                                        .color(p.muted),
                                    );
                                }
                            }
                            if let Some(err) = &self.vault_error {
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new(err.as_str()).color(p.danger));
                            }
                            if let Ok(status) = self.ctx.note_status.lock() {
                                match &*status {
                                    NoteStatus::Idle => {}
                                    NoteStatus::Written(path) => {
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Note écrite : {}",
                                                path.display()
                                            ))
                                            .size(11.5)
                                            .color(p.ok),
                                        );
                                    }
                                    NoteStatus::Failed(err) => {
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(err.as_str()).color(p.danger),
                                        );
                                    }
                                }
                            }
                        });

                        ui.add_space(12.0);

                        // Carte « Notes récentes ».
                        card_frame(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            section_title(ui, &p, "Notes récentes · AudioNotes");
                            // N'inclut jamais les entrées en mode Daily Note : elles vivent
                            // dans un fichier partagé par jour, sans .wav associé en 1 pour
                            // 1 (voir vault::dashboard).
                            let mut play_request: Option<PathBuf> = None;
                            let mut stop_request = false;
                            if let Ok(vault_guard) = self.ctx.vault.lock() {
                                match vault_guard.as_ref() {
                                    Some(vault) => {
                                        let notes =
                                            obsidianrecorder_lib::vault::dashboard::list_recent_notes(
                                                &vault.resolve(AUDIO_NOTES_SUBFOLDER),
                                                5,
                                            );
                                        if notes.is_empty() {
                                            ui.label(
                                                egui::RichText::new(
                                                    "Aucune note pour le moment.",
                                                )
                                                .size(12.0)
                                                .color(p.muted),
                                            );
                                        } else {
                                            for note in &notes {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} — {}",
                                                            note.date_label
                                                                .as_deref()
                                                                .unwrap_or("date inconnue"),
                                                            note.duration_label
                                                                .as_deref()
                                                                .unwrap_or("durée inconnue"),
                                                        ))
                                                        .size(12.0)
                                                        .color(p.text),
                                                    );
                                                    let playing = self.playback.is_playing();
                                                    let label =
                                                        if playing { "Pause" } else { "Écouter" };
                                                    let small_button = egui::Button::new(
                                                        egui::RichText::new(label)
                                                            .size(11.5)
                                                            .color(p.accent),
                                                    )
                                                    .fill(p.accent_soft)
                                                    .stroke(egui::Stroke::new(1.0, p.accent))
                                                    .rounding(egui::Rounding::same(8.0));
                                                    if ui.add(small_button).clicked() {
                                                        if playing {
                                                            stop_request = true;
                                                        } else if let Some(stem) = note
                                                            .path
                                                            .file_stem()
                                                            .and_then(|s| s.to_str())
                                                        {
                                                            play_request = find_attachment(
                                                                &vault.resolve(ATTACHMENTS_SUBFOLDER),
                                                                stem,
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    }
                                    None => {
                                        ui.label(
                                            egui::RichText::new(
                                                "Configurez un Vault pour voir vos notes récentes ici.",
                                            )
                                            .size(12.0)
                                            .color(p.muted),
                                        );
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
                        });

                        ui.add_space(12.0);

                        // Carte « Réglages » (repliée par défaut).
                        card_frame(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.checkbox(
                                &mut self.show_settings,
                                egui::RichText::new("Réglages").color(p.text),
                            );

                            if self.show_settings {
                                ui.add_space(10.0);
                                if let Ok(mut settings) = self.ctx.settings.lock() {
                                    ui.label(
                                        egui::RichText::new("Mode d'écriture des notes")
                                            .size(12.0)
                                            .color(p.muted),
                                    );
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

                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new("Thème")
                                            .size(12.0)
                                            .color(p.muted),
                                    );
                                    let mut theme_changed = false;
                                    theme_changed |= ui
                                        .radio_value(
                                            &mut settings.theme,
                                            ThemePreference::System,
                                            "Système",
                                        )
                                        .clicked();
                                    theme_changed |= ui
                                        .radio_value(
                                            &mut settings.theme,
                                            ThemePreference::Light,
                                            "Clair",
                                        )
                                        .clicked();
                                    theme_changed |= ui
                                        .radio_value(
                                            &mut settings.theme,
                                            ThemePreference::Dark,
                                            "Sombre",
                                        )
                                        .clicked();

                                    if mode_changed || theme_changed {
                                        let _ = settings::save(&settings);
                                    }

                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new("Seuil de captation (anti-silence)")
                                            .size(12.0)
                                            .color(p.muted),
                                    );
                                    let threshold_changed = ui
                                        .add(
                                            egui::Slider::new(
                                                &mut settings.capture_threshold,
                                                0.0..=0.3,
                                            )
                                            .text("seuil")
                                            .fixed_decimals(3),
                                        )
                                        .changed();
                                    ui.label(
                                        egui::RichText::new(
                                            "0 = désactivé (tout est enregistré). Au-delà de 0, \
                                             l'enregistrement se met en pause pendant les silences \
                                             prolongés, avec un temps de maintien d'environ 1,5 s \
                                             pour ne pas couper les fins de mots — les blancs ne \
                                             sont jamais encodés, ce qui réduit la taille du \
                                             fichier final. Le repère rouge sur le vu-mètre, \
                                             pendant un enregistrement, indique le niveau \
                                             actuellement réglé.",
                                        )
                                        .size(11.0)
                                        .color(p.muted),
                                    );
                                    if threshold_changed {
                                        let _ = settings::save(&settings);
                                    }

                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Tags supplémentaires (séparés par des virgules)",
                                        )
                                        .size(12.0)
                                        .color(p.muted),
                                    );
                                    if ui.text_edit_singleline(&mut self.tags_input).changed() {
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
                        });

                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(self.tray_status.as_str())
                                .size(11.0)
                                .color(p.muted),
                        );
                    });
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
            // Le style de la fenêtre principale (déjà appliqué ci-dessus au
            // contexte partagé) vaut aussi pour cette viewport immédiate :
            // l'overlay hérite donc de la même palette, sans re-calcul.
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("obsidianrecorder-level-overlay"),
                egui::ViewportBuilder::default()
                    .with_title("ObsidianRECORDER")
                    .with_inner_size([240.0, 78.0])
                    .with_always_on_top()
                    .with_decorations(false)
                    .with_resizable(false),
                |overlay_ctx, class| {
                    if class != egui::ViewportClass::Immediate {
                        return;
                    }
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::none()
                                .fill(p.card)
                                .inner_margin(egui::Margin::same(12.0)),
                        )
                        .show(overlay_ctx, |ui| {
                            ui.horizontal(|ui| {
                                let (dot, label) = if gate_open {
                                    (p.danger, "Enregistrement en cours")
                                } else {
                                    (p.muted, "En pause (silence)")
                                };
                                ui.label(egui::RichText::new("●").size(12.0).color(dot));
                                ui.label(
                                    egui::RichText::new(label)
                                        .size(12.0)
                                        .strong()
                                        .color(p.text),
                                );
                            });
                            ui.add_space(8.0);
                            meter(ui, &p, display_level, Some(threshold_marker));
                        });
                },
            );
        }
    }
}

/// Dossier où sont déposés les enregistrements (et les imports convertis).
fn default_recordings_dir() -> std::io::Result<PathBuf> {
    let dir = paths::fallback_recordings_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Retrouve le premier fichier audio associé à une note dans `attachments/`.
/// Selon le format réglé et la découpe, il peut s'appeler `rec_X.mp3`,
/// `rec_X.wav`, `rec_X_part1.mp3`, etc.
fn find_attachment(attachments_dir: &Path, stem: &str) -> Option<PathBuf> {
    [
        format!("{stem}.mp3"),
        format!("{stem}.wav"),
        format!("{stem}_part1.mp3"),
        format!("{stem}_part1.wav"),
    ]
    .iter()
    .map(|name| attachments_dir.join(name))
    .find(|path| path.is_file())
}

/// Bascule l'enregistrement (démarre si arrêté, arrête et finalise sinon).
/// Déclenche automatiquement transcription et écriture de note en tâche de
/// fond dès que l'arrêt produit un ou plusieurs fichiers audio.
///
/// Partagée entre le clic du bouton, le menu System Tray et le raccourci
/// global (threads différents, voir [`setup_global_hotkey`]). Lit le
/// périphérique d'entrée, le seuil de captation, le format et la taille de
/// découpe directement dans `ctx.settings` (plutôt que de les recevoir en
/// paramètre) afin que les trois déclencheurs utilisent toujours exactement
/// les mêmes réglages que ceux affichés dans l'interface — avant ce
/// correctif, le raccourci clavier ignorait silencieusement le microphone
/// sélectionné et retombait sur le périphérique par défaut du système.
fn toggle_recording(ctx: &AppContext) -> AudioResult<()> {
    let mut capture = ctx
        .audio
        .lock()
        .map_err(|_| AudioError::BuildStream("état audio verrouillé de façon incohérente".into()))?;

    if capture.is_recording() {
        let summary = capture.stop()?;
        let text = format!(
            "{} ({:.1} s)",
            join_paths(&summary.paths),
            summary.duration.as_secs_f64()
        );
        if let Ok(mut last) = ctx.last_recording.lock() {
            *last = Some(text);
        }
        drop(capture);
        spawn_transcription_and_note(ctx.clone(), summary);
        return Ok(());
    }

    let (device, threshold, format, max_bytes) = ctx
        .settings
        .lock()
        .map(|s| {
            (
                s.input_device.clone(),
                s.capture_threshold,
                format_from_setting(s.output_format),
                audio::chunk_bytes_from_mb(s.chunk_size_mb),
            )
        })
        .unwrap_or((None, 0.0, RecordingFormat::Mp3, None));
    let output_dir = default_recordings_dir().map_err(|e| AudioError::BuildStream(e.to_string()))?;

    capture.start(CaptureConfig {
        device_name: device,
        output_dir,
        file_stem: paths::timestamped_stem(),
        silence_threshold: threshold,
        format,
        max_chunk_bytes: max_bytes,
    })
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

/// Transcrit un enregistrement fraîchement terminé (un ou plusieurs
/// morceaux : voir la découpe dans `audio::encoder`) puis, si un Vault est
/// configuré, écrit automatiquement la note Markdown correspondante selon
/// le mode et les tags actuellement réglés. Tout se passe en tâche de fond,
/// sans bloquer l'interface.
///
/// L'audio peut être du MP3 (format par défaut) ou du WAV : il est décodé
/// en mémoire vers le format attendu par Whisper (mono 16 kHz) via
/// [`audio::decode_to_mono_16k`], puis transcrit morceau par morceau.
fn spawn_transcription_and_note(ctx: AppContext, summary: RecordingSummary) {
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

        let mut full_text = String::new();
        let mut failure: Option<String> = None;
        for path in &summary.paths {
            let samples = match audio::decode_to_mono_16k(path) {
                Ok(samples) => samples,
                Err(err) => {
                    failure = Some(err.to_string());
                    break;
                }
            };
            match transcriber.transcribe(&samples) {
                Ok(transcript) => {
                    let part = transcript.full_text();
                    if !part.is_empty() {
                        if !full_text.is_empty() {
                            full_text.push(' ');
                        }
                        full_text.push_str(&part);
                    }
                }
                Err(err) => {
                    failure = Some(err.to_string());
                    break;
                }
            }
        }

        if let Ok(mut guard) = ctx.stt.lock() {
            *guard = match &failure {
                Some(err) => SttState::Failed(err.clone()),
                None => SttState::Done(full_text.clone()),
            };
        }
        if failure.is_some() {
            return;
        }

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
            &summary.paths,
            &full_text,
            summary.duration,
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

/// Copie le(s) fichier(s) audio dans `attachments/`, construit la note
/// Markdown (frontmatter + transcription + lecture intégrée) et l'écrit
/// selon le mode choisi dans les réglages (nouvelle note dans
/// `AudioNotes/`, ou ajout à la Daily Note du jour dans `Daily Notes/`).
fn write_note_to_vault(
    vault: &Vault,
    audio_paths: &[PathBuf],
    transcript_text: &str,
    duration: Duration,
    write_mode_setting: WriteModeSetting,
    extra_tags: Vec<String>,
) -> Result<PathBuf, String> {
    let attachments_dir = vault.resolve(ATTACHMENTS_SUBFOLDER);
    std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;

    let mut audio_files = Vec::with_capacity(audio_paths.len());
    for audio_path in audio_paths {
        let filename = audio_path
            .file_name()
            .ok_or_else(|| "nom de fichier audio invalide".to_string())?;
        std::fs::copy(audio_path, attachments_dir.join(filename)).map_err(|e| e.to_string())?;
        audio_files.push(format!(
            "{ATTACHMENTS_SUBFOLDER}/{}",
            filename.to_string_lossy()
        ));
    }

    let note_data = NoteData {
        frontmatter: FrontmatterData {
            date: chrono::Local::now(),
            duration,
            extra_tags,
            audio_files,
        },
        transcript_text: transcript_text.to_string(),
    };

    let content = obsidianrecorder_lib::vault::note::render_note(&note_data);

    // Le nom de la note reprend l'horodatage de l'enregistrement d'origine
    // (même racine que l'audio), pour que les deux restent faciles à
    // rapprocher même en dehors du frontmatter. Le suffixe `_part1` d'un
    // enregistrement découpé est retiré pour un nom de note plus lisible.
    // Ignoré en mode Daily Note (voir `vault::writer::WriteMode`).
    let stem = audio_paths
        .first()
        .and_then(|path| path.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("note");
    let stem = stem.strip_suffix("_part1").unwrap_or(stem);
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
                    if let Err(err) = toggle_recording(&ctx) {
                        let message = format!(
                            "[obsidianrecorder] échec de la bascule d'enregistrement (raccourci) : {err}"
                        );
                        eprintln!("{message}");
                        // Ce thread n'a pas accès à l'état de l'interface :
                        // sans ceci, une erreur ici (ex. microphone occupé)
                        // n'avait jusqu'ici aucun retour visible pour
                        // l'utilisateur — l'enregistrement ne démarrait
                        // tout simplement pas, silencieusement. `update()`
                        // relit cette file et l'affiche au prochain repaint.
                        if let Ok(mut shared) = ctx.background_error.lock() {
                            *shared = Some(err.to_string());
                        }
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
