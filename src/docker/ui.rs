//! Services tool window: containers, images, compose, logs.

use super::api::{self, Cmd, ContainerRow, DockerApi, Event, ImageRow};
use crate::theme::Theme;
use egui::RichText;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Containers,
    Images,
    Compose,
}

pub enum DockerPick {
    /// Open a terminal tab running this command (docker exec …).
    Terminal(Vec<String>),
}

pub struct DockerPanel {
    api: Option<DockerApi>,
    root: PathBuf,
    tab: Tab,
    containers: Vec<ContainerRow>,
    images: Vec<ImageRow>,
    compose: Vec<PathBuf>,
    logs_for: Option<String>,
    logs: Vec<String>,
    log_filter: String,
    console: Vec<String>,
    pull_name: String,
    error: Option<String>,
    busy: usize,
    last_refresh: Instant,
}

impl DockerPanel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            api: None,
            compose: api::compose_files(&root),
            root,
            tab: Tab::Containers,
            containers: Vec::new(),
            images: Vec::new(),
            logs_for: None,
            logs: Vec::new(),
            log_filter: String::new(),
            console: Vec::new(),
            pull_name: String::new(),
            error: None,
            busy: 0,
            last_refresh: Instant::now() - Duration::from_secs(60),
        }
    }

    fn send(&mut self, cmd: Cmd) {
        if let Some(api) = &self.api {
            self.busy += 1;
            api.send(cmd);
        }
    }

    pub fn tick(&mut self, ctx: &egui::Context, visible: bool) {
        if !visible {
            return;
        }
        if self.api.is_none() {
            self.api = Some(DockerApi::start(self.root.clone(), ctx.clone()));
        }
        let mut events = Vec::new();
        if let Some(api) = &self.api {
            while let Ok(e) = api.rx.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                Event::Containers(c) => self.containers = c,
                Event::Images(i) => self.images = i,
                Event::Log(l) => {
                    self.logs.push(l);
                    if self.logs.len() > 5000 {
                        self.logs.drain(..1000);
                    }
                }
                Event::LogsCleared => self.logs.clear(),
                Event::Console(l) => self.console.push(l),
                Event::Error(e) => {
                    self.console.push(format!("error: {e}"));
                    self.error = Some(e);
                }
                Event::Done => {
                    self.busy = self.busy.saturating_sub(1);
                    if self.busy == 0 {
                        self.last_refresh = Instant::now() - Duration::from_secs(60);
                    }
                }
            }
        }
        if self.busy == 0 && self.last_refresh.elapsed() > Duration::from_secs(3) {
            self.last_refresh = Instant::now();
            self.send(Cmd::Refresh);
        }
        ctx.request_repaint_after(Duration::from_secs(3));
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) -> Option<DockerPick> {
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut pick = None;
        let mut cmds: Vec<Cmd> = Vec::new();
        ui.horizontal(|ui| {
            for (t, name) in [(Tab::Containers, "Containers"), (Tab::Images, "Images"), (Tab::Compose, "Compose")] {
                let label = match t {
                    Tab::Containers => format!("{name} ({})", self.containers.len()),
                    Tab::Images => format!("{name} ({})", self.images.len()),
                    Tab::Compose => format!("{name} ({})", self.compose.len()),
                };
                if ui.selectable_label(self.tab == t, label).clicked() {
                    self.tab = t;
                }
            }
            if ui.small_button("⟳").clicked() {
                cmds.push(Cmd::Refresh);
            }
            if self.busy > 0 {
                ui.spinner();
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme.c(&theme.ui.error)));
            }
        });
        ui.separator();
        match self.tab {
            Tab::Containers => {
                egui::Panel::left("docker-containers").default_size(640.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        egui::Grid::new("docker-grid").striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                            for h in ["", "Name", "Image", "Status", "Ports", ""] {
                                ui.label(RichText::new(h).strong().color(accent));
                            }
                            ui.end_row();
                            for c in self.containers.clone() {
                                let running = c.state == "running";
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                let color = if running { theme.c(&theme.ui.success) } else { muted };
                                if running {
                                    ui.painter().circle_filled(dot.center(), 4.0, color);
                                } else {
                                    ui.painter().circle_stroke(dot.center(), 4.0, egui::Stroke::new(1.0, color));
                                }
                                let r = ui.selectable_label(self.logs_for.as_deref() == Some(&c.id), RichText::new(&c.name).monospace());
                                if r.clicked() {
                                    self.logs_for = Some(c.id.clone());
                                    cmds.push(Cmd::Logs(c.id.clone()));
                                }
                                ui.label(RichText::new(&c.image).small());
                                ui.label(RichText::new(&c.status).small().color(muted));
                                ui.label(RichText::new(&c.ports).small().monospace());
                                ui.horizontal(|ui| {
                                    if running {
                                        if ui.small_button("Stop").clicked() {
                                            cmds.push(Cmd::Stop(c.id.clone()));
                                        }
                                        if ui.small_button("Restart").clicked() {
                                            cmds.push(Cmd::Restart(c.id.clone()));
                                        }
                                        if ui.small_button("Exec").on_hover_text("Open a shell in the container").clicked() {
                                            pick = Some(DockerPick::Terminal(vec![
                                                "docker".into(),
                                                "exec".into(),
                                                "-it".into(),
                                                c.id.clone(),
                                                "/bin/sh".into(),
                                            ]));
                                        }
                                    } else if ui.small_button("Start").clicked() {
                                        cmds.push(Cmd::Start(c.id.clone()));
                                    }
                                    if ui.small_button("Remove").clicked() {
                                        cmds.push(Cmd::Remove(c.id.clone()));
                                    }
                                });
                                ui.end_row();
                            }
                        });
                        if self.containers.is_empty() {
                            ui.label(RichText::new("No containers").color(muted));
                        }
                    });
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let name = self
                            .logs_for
                            .as_ref()
                            .and_then(|id| self.containers.iter().find(|c| &c.id == id))
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| "logs".into());
                        ui.label(RichText::new(format!("Log: {name}")).strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.log_filter)
                                .hint_text("filter")
                                .desired_width(200.0),
                        );
                        if ui.small_button("Clear").clicked() {
                            self.logs.clear();
                        }
                    });
                    egui::ScrollArea::both()
                        .id_salt("docker-logs")
                        .auto_shrink(false)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            let f = self.log_filter.to_lowercase();
                            for l in self.logs.iter().filter(|l| f.is_empty() || l.to_lowercase().contains(&f)) {
                                ui.add(egui::Label::new(RichText::new(l).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
                            }
                        });
                });
            }
            Tab::Images => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pull_name)
                            .hint_text("image:tag")
                            .desired_width(260.0),
                    );
                    if ui.button("Pull").clicked() && !self.pull_name.trim().is_empty() {
                        cmds.push(Cmd::Pull(self.pull_name.trim().to_owned()));
                    }
                });
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    egui::Grid::new("docker-images").striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                        for h in ["Tags", "Id", "Size", "Created", ""] {
                            ui.label(RichText::new(h).strong().color(accent));
                        }
                        ui.end_row();
                        for i in self.images.clone() {
                            ui.label(RichText::new(&i.tags).monospace());
                            ui.label(
                                RichText::new(i.id.trim_start_matches("sha256:").chars().take(12).collect::<String>())
                                    .small()
                                    .color(muted),
                            );
                            ui.label(api::human_size(i.size));
                            ui.label(RichText::new(crate::git::cmd::fmt_date(i.created)).small().color(muted));
                            if ui.small_button("Remove").clicked() {
                                cmds.push(Cmd::RemoveImage(i.id.clone()));
                            }
                            ui.end_row();
                        }
                    });
                    for l in self.console.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
                        ui.label(RichText::new(l).small().color(muted));
                    }
                });
            }
            Tab::Compose => {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    if self.compose.is_empty() {
                        ui.label(RichText::new("No docker-compose*.yml in the project").color(muted));
                    }
                    for f in self.compose.clone() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(f.strip_prefix(&self.root).unwrap_or(&f).display().to_string()).monospace());
                            for (label, args) in [
                                ("Up", vec!["up", "-d"]),
                                ("Down", vec!["down"]),
                                ("Ps", vec!["ps"]),
                                ("Restart", vec!["restart"]),
                                ("Logs", vec!["logs", "--tail", "100"]),
                            ] {
                                if ui.small_button(label).clicked() {
                                    cmds.push(Cmd::Compose {
                                        file: f.clone(),
                                        args: args.iter().map(|s| s.to_string()).collect(),
                                    });
                                }
                            }
                        });
                    }
                    ui.separator();
                    for l in &self.console {
                        ui.add(egui::Label::new(RichText::new(l).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
                    }
                });
            }
        }
        for c in cmds {
            self.send(c);
        }
        pick
    }
}
