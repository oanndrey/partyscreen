// Tela P2P: app para compartilhar a tela com um código (estilo Discord) e assistir
// quem está compartilhando. Também funciona com quem assiste pelo navegador.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod capture;
#[cfg(target_os = "macos")]
#[path = "capture_macos.rs"]
mod capture;
#[cfg(any(windows, target_os = "macos"))]
mod encoder;
mod audio;
mod gui;
mod session;
mod theme;
mod viewer;

/// Quadro H.264 codificado pronto para enviar
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub duration: std::time::Duration,
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Tela P2P")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([420.0, 420.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("ícone")),
        ..Default::default()
    };
    eframe::run_native("Tela P2P", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
