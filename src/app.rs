use crate::theme::{self, Theme};
use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use std::sync::Arc;

pub struct App {
    pub theme: Theme,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let theme = theme::load("palenight");
        theme.apply(&cc.egui_ctx);
        Self { theme }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "jbmono".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "jbmono-bold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"))),
    );
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jbmono".into());
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
        style.text_styles.insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(13.0, FontFamily::Proportional));
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open Folder…").clicked() {
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.label("Theme");
                    for n in theme::names() {
                        if ui.selectable_label(self.theme.name == theme::load(n).name, n).clicked() {
                            self.theme = theme::load(n);
                            self.theme.apply(&ctx);
                            ui.close();
                        }
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.theme.name);
            });
        });

        egui::Panel::left("project").default_size(240.0).show(ui, |ui| {
            ui.heading("Project");
            ui.label("No folder open");
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("Open a folder to start  (⌘⇧O)").color(self.theme.c(&self.theme.ui.fg_muted)));
            });
        });
    }
}
