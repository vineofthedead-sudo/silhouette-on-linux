//! Silhouette Studio for Linux: desktop application.

mod app;
mod canvas;
mod panels;
mod mock_send;
// Retain the legacy sender for later hardware work; the V5 dock is mock-only.
#[allow(dead_code)]
mod send;

use std::path::PathBuf;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let file = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Silhouette Studio")
            .with_app_id("silhouette-studio")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("Silhouette Studio", options, Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, file)))))
}
