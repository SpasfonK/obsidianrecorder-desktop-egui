// Masque la console au lancement en mode release (l'utilisateur final ne
// doit pas voir de fenêtre de terminal derrière l'application), tout en la
// conservant en mode debug pour suivre les journaux du socle audio et
// hotkey (`println!`/`eprintln!`). Sans effet sur les autres plateformes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use eframe::egui;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 640.0])
            .with_min_inner_size([380.0, 480.0])
            .with_icon(load_icon())
            .with_title("ObsidianRECORDER Desktop"),
        ..Default::default()
    };

    eframe::run_native(
        "ObsidianRECORDER Desktop",
        native_options,
        Box::new(|cc| Ok(Box::new(app::ObsidianRecorderApp::new(cc)))),
    )
}

/// Charge l'icône embarquée dans le binaire (aucun fichier externe à
/// distribuer à côté de l'exécutable).
fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../icons/icon.png");
    let image = image::load_from_memory(bytes)
        .expect("l'icône intégrée doit être un PNG valide")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
