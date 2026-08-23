//! Ruche — plusieurs clients Minecraft cote a cote, sans saturer la machine.
//!
//! Les versions proposees sont celles deja installees dans `.minecraft/versions`.

// Pas de console noire derriere la fenetre en release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use ruche::app;

/// L'icone est stockee en RGBA brut : pas besoin d'un decodeur d'images.
fn app_icon() -> egui::IconData {
    const SIDE: u32 = 64;
    const PIXELS: &[u8] = include_bytes!("../assets/icon_64.rgba");
    egui::IconData {
        rgba: PIXELS.to_vec(),
        width: SIDE,
        height: SIDE,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([960.0, 620.0])
            .with_icon(app_icon())
            .with_title("Ruche"),
        ..Default::default()
    };
    eframe::run_native(
        "Ruche",
        options,
        Box::new(|cc| {
            app::apply_theme(&cc.egui_ctx);
            Ok(Box::new(app::App::new(&cc.egui_ctx)))
        }),
    )
}
