#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("code-editor")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("code-editor", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
