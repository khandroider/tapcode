#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod db;
mod docker;
mod editor;
mod git;
mod index;
mod keymap;
mod lsp;
mod search_ui;
mod settings;
mod terminal;
mod theme;
mod workspace;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("tapcode")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    // `tapcode [folder] [file...]` — like `idea .`
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    eframe::run_native("tapcode", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, &args)))))
}
