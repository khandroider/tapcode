//! Services tool window (JetBrains layout): tree of compose projects / containers / images / networks /
//! volumes on the left; the selected container's Log · Dashboard · Terminal on the right.

use super::api::{self, Cmd, ContainerRow, DockerApi, Event, ImageRow, NetworkRow, VolumeRow};
use crate::terminal::pty::{Dims, Session};
use crate::theme::Theme;
use egui::RichText;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Log,
    Dashboard,
    Terminal,
}

#[derive(Clone, PartialEq, Eq)]
enum Selection {
    Container(String),
    Image(String),
    Compose(String),
}

pub struct DockerPanel {
    api: Option<DockerApi>,
    root: PathBuf,
    containers: Vec<ContainerRow>,
    images: Vec<ImageRow>,
    networks: Vec<NetworkRow>,
    volumes: Vec<VolumeRow>,
    compose: Vec<PathBuf>,
    selected: Option<Selection>,
    tab: RightTab,
    logs_for: Option<String>,
    logs: Vec<String>,
    log_filter: String,
    inspect: HashMap<String, Vec<(String, String)>>,
    exec: HashMap<String, Session>,
    console: Vec<String>,
    pull_name: String,
    error: Option<String>,
    busy: usize,
    last_refresh: Instant,
    collapsed: HashSet<String>,
}

impl DockerPanel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            api: None,
            compose: api::compose_files(&root),
            root,
            containers: Vec::new(),
            images: Vec::new(),
            networks: Vec::new(),
            volumes: Vec::new(),
            selected: None,
            tab: RightTab::Log,
            logs_for: None,
            logs: Vec::new(),
            log_filter: String::new(),
            inspect: HashMap::new(),
            exec: HashMap::new(),
            console: Vec::new(),
            pull_name: String::new(),
            error: None,
            busy: 0,
            last_refresh: Instant::now() - Duration::from_secs(60),
            collapsed: HashSet::new(),
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
                Event::Networks(n) => self.networks = n,
                Event::Volumes(v) => self.volumes = v,
                Event::Log(l) => {
                    self.logs.push(l);
                    if self.logs.len() > 5000 {
                        self.logs.drain(..1000);
                    }
                }
                Event::LogsCleared => self.logs.clear(),
                Event::Inspect(id, props) => {
                    self.inspect.insert(id, props);
                }
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

    fn select_container(&mut self, id: &str) {
        self.selected = Some(Selection::Container(id.to_owned()));
        if self.logs_for.as_deref() != Some(id) {
            self.logs_for = Some(id.to_owned());
            self.send(Cmd::Logs(id.to_owned()));
        }
        if !self.inspect.contains_key(id) {
            self.send(Cmd::Inspect(id.to_owned()));
        }
    }

    fn compose_file_for(&self, project: &str) -> Option<PathBuf> {
        self.containers
            .iter()
            .find(|c| c.compose_project.as_deref() == Some(project))
            .and_then(|c| c.compose_files.clone())
            .and_then(|f| f.split(',').next().map(PathBuf::from))
            .filter(|p| p.exists())
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let muted = theme.c(&theme.ui.fg_muted);
        let accent = theme.c(&theme.ui.accent);
        let mut cmds: Vec<Cmd> = Vec::new();
        let mut select: Option<String> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Services").strong());
            if ui.small_button("⟳").clicked() {
                cmds.push(Cmd::Refresh);
            }
            if self.busy > 0 {
                ui.spinner();
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme.c(&theme.ui.error)).small());
            }
        });
        ui.separator();

        // ---- left: tree
        egui::Panel::left("docker-tree").default_size(380.0).show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("docker-tree-scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    let node = |ui: &mut egui::Ui,
                                collapsed: &mut HashSet<String>,
                                key: &str,
                                indent: f32,
                                text: RichText,
                                count: Option<usize>|
                     -> (bool, egui::Response) {
                        let open = !collapsed.contains(key);
                        let r = ui.horizontal(|ui| {
                            ui.add_space(indent);
                            let a = ui.add(
                                egui::Label::new(RichText::new(if open { "▾" } else { "▸" }).color(muted)).sense(egui::Sense::click()),
                            );
                            let r = ui.selectable_label(false, text);
                            if let Some(n) = count {
                                ui.label(RichText::new(n.to_string()).small().color(theme.c(&theme.ui.warning)));
                            }
                            if a.clicked() {
                                if open {
                                    collapsed.insert(key.to_owned());
                                } else {
                                    collapsed.remove(key);
                                }
                            }
                            r
                        });
                        (!collapsed.contains(key), r.inner)
                    };
                    let (open, _) = node(ui, &mut self.collapsed, "docker", 0.0, RichText::new("Docker").strong(), None);
                    if !open {
                        return;
                    }
                    // Compose projects.
                    let mut projects: Vec<String> = self.containers.iter().filter_map(|c| c.compose_project.clone()).collect();
                    projects.sort();
                    projects.dedup();
                    for project in projects {
                        let key = format!("compose:{project}");
                        let (open, r) = node(
                            ui,
                            &mut self.collapsed,
                            &key,
                            18.0,
                            RichText::new(format!("Docker-compose: {project}")).color(accent),
                            None,
                        );
                        if r.clicked() {
                            self.selected = Some(Selection::Compose(project.clone()));
                        }
                        let file = self.compose_file_for(&project);
                        r.context_menu(|ui| {
                            for (label, args) in [
                                ("Up", vec!["up", "-d"]),
                                ("Down", vec!["down"]),
                                ("Restart", vec!["restart"]),
                                ("Ps", vec!["ps"]),
                            ] {
                                if ui.add_enabled(file.is_some(), egui::Button::new(label)).clicked()
                                    && let Some(f) = &file
                                {
                                    cmds.push(Cmd::Compose {
                                        file: f.clone(),
                                        args: args.iter().map(|s| s.to_string()).collect(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                        if !open {
                            continue;
                        }
                        let mut services: Vec<String> = self
                            .containers
                            .iter()
                            .filter(|c| c.compose_project.as_deref() == Some(&project))
                            .filter_map(|c| c.compose_service.clone())
                            .collect();
                        services.sort();
                        services.dedup();
                        for service in services {
                            let skey = format!("{key}/{service}");
                            let (sopen, _) = node(ui, &mut self.collapsed, &skey, 36.0, RichText::new(&service), None);
                            if !sopen {
                                continue;
                            }
                            let items: Vec<ContainerRow> = self
                                .containers
                                .iter()
                                .filter(|c| {
                                    c.compose_project.as_deref() == Some(&project) && c.compose_service.as_deref() == Some(&service)
                                })
                                .cloned()
                                .collect();
                            for c in items {
                                if let Some(id) = container_row(
                                    ui,
                                    &c,
                                    54.0,
                                    self.selected == Some(Selection::Container(c.id.clone())),
                                    theme,
                                    &mut cmds,
                                ) {
                                    select = Some(id);
                                }
                            }
                        }
                    }
                    // Standalone containers.
                    let standalone: Vec<ContainerRow> = self.containers.iter().filter(|c| c.compose_project.is_none()).cloned().collect();
                    let (open, _) = node(
                        ui,
                        &mut self.collapsed,
                        "containers",
                        18.0,
                        RichText::new("Containers"),
                        Some(standalone.len()),
                    );
                    if open {
                        for c in standalone {
                            if let Some(id) = container_row(
                                ui,
                                &c,
                                36.0,
                                self.selected == Some(Selection::Container(c.id.clone())),
                                theme,
                                &mut cmds,
                            ) {
                                select = Some(id);
                            }
                        }
                    }
                    // Images.
                    let (open, _) = node(
                        ui,
                        &mut self.collapsed,
                        "images",
                        18.0,
                        RichText::new("Images"),
                        Some(self.images.len()),
                    );
                    if open {
                        ui.horizontal(|ui| {
                            ui.add_space(36.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.pull_name)
                                    .hint_text("image:tag")
                                    .desired_width(160.0),
                            );
                            if ui.small_button("Pull").clicked() && !self.pull_name.trim().is_empty() {
                                cmds.push(Cmd::Pull(self.pull_name.trim().to_owned()));
                            }
                        });
                        for i in self.images.clone() {
                            let r = ui.horizontal(|ui| {
                                ui.add_space(36.0);
                                let r = ui.selectable_label(
                                    self.selected == Some(Selection::Image(i.id.clone())),
                                    RichText::new(&i.tags).monospace(),
                                );
                                ui.label(RichText::new(api::human_size(i.size)).small().color(muted));
                                r
                            });
                            if r.inner.clicked() {
                                self.selected = Some(Selection::Image(i.id.clone()));
                            }
                            r.inner.context_menu(|ui| {
                                if ui.button("Remove image").clicked() {
                                    cmds.push(Cmd::RemoveImage(i.id.clone()));
                                    ui.close();
                                }
                            });
                        }
                    }
                    // Networks / volumes.
                    let (open, _) = node(
                        ui,
                        &mut self.collapsed,
                        "networks",
                        18.0,
                        RichText::new("Networks"),
                        Some(self.networks.len()),
                    );
                    if open {
                        for n in &self.networks {
                            ui.horizontal(|ui| {
                                ui.add_space(36.0);
                                ui.label(RichText::new(&n.name).monospace());
                                ui.label(RichText::new(format!("{} · {}", n.driver, n.scope)).small().color(muted));
                            });
                        }
                    }
                    let (open, _) = node(
                        ui,
                        &mut self.collapsed,
                        "volumes",
                        18.0,
                        RichText::new("Volumes"),
                        Some(self.volumes.len()),
                    );
                    if open {
                        for v in &self.volumes {
                            ui.horizontal(|ui| {
                                ui.add_space(36.0);
                                ui.label(RichText::new(&v.name).monospace());
                                ui.label(RichText::new(&v.driver).small().color(muted));
                            })
                            .response
                            .on_hover_text(&v.mountpoint);
                        }
                    }
                    if self.containers.is_empty() && self.images.is_empty() {
                        ui.label(RichText::new("Nothing from the Docker daemon yet…").color(muted));
                    }
                });
        });

        // ---- right: details
        egui::CentralPanel::default().show(ui, |ui| match self.selected.clone() {
            Some(Selection::Container(id)) => self.container_details(ui, theme, &id, &mut cmds),
            Some(Selection::Image(id)) => {
                if let Some(i) = self.images.iter().find(|i| i.id == id).cloned() {
                    ui.label(RichText::new(&i.tags).strong());
                    ui.label(
                        RichText::new(format!(
                            "{}  ·  {}  ·  {}",
                            i.id,
                            api::human_size(i.size),
                            crate::git::cmd::fmt_date(i.created)
                        ))
                        .color(muted),
                    );
                    if ui.button("Remove image").clicked() {
                        cmds.push(Cmd::RemoveImage(i.id.clone()));
                    }
                }
            }
            Some(Selection::Compose(project)) => {
                ui.label(RichText::new(format!("Docker-compose: {project}")).strong());
                let file = self.compose_file_for(&project);
                ui.label(
                    RichText::new(
                        file.as_ref()
                            .map(|f| f.display().to_string())
                            .unwrap_or_else(|| "compose file not found".into()),
                    )
                    .color(muted),
                );
                ui.horizontal(|ui| {
                    for (label, args) in [
                        ("Up", vec!["up", "-d"]),
                        ("Down", vec!["down"]),
                        ("Restart", vec!["restart"]),
                        ("Ps", vec!["ps"]),
                        ("Logs", vec!["logs", "--tail", "200"]),
                    ] {
                        if ui.add_enabled(file.is_some(), egui::Button::new(label)).clicked()
                            && let Some(f) = &file
                        {
                            cmds.push(Cmd::Compose {
                                file: f.clone(),
                                args: args.iter().map(|s| s.to_string()).collect(),
                            });
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.console {
                            ui.add(egui::Label::new(RichText::new(l).monospace().small()).wrap_mode(egui::TextWrapMode::Extend));
                        }
                    });
            }
            None => {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(RichText::new("Select a container to see its log, dashboard and terminal").color(muted));
                    if !self.compose.is_empty() {
                        ui.add_space(12.0);
                        ui.label(RichText::new("Compose files in this project:").color(muted));
                        for f in self.compose.clone() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(f.strip_prefix(&self.root).unwrap_or(&f).display().to_string()).monospace());
                                if ui.small_button("Up").clicked() {
                                    cmds.push(Cmd::Compose {
                                        file: f.clone(),
                                        args: vec!["up".into(), "-d".into()],
                                    });
                                }
                                if ui.small_button("Down").clicked() {
                                    cmds.push(Cmd::Compose {
                                        file: f.clone(),
                                        args: vec!["down".into()],
                                    });
                                }
                            });
                        }
                    }
                });
            }
        });

        if let Some(id) = select {
            self.select_container(&id);
        }
        for c in cmds {
            self.send(c);
        }
    }

    fn container_details(&mut self, ui: &mut egui::Ui, theme: &Theme, id: &str, cmds: &mut Vec<Cmd>) {
        let muted = theme.c(&theme.ui.fg_muted);
        let Some(c) = self.containers.iter().find(|c| c.id == id).cloned() else {
            ui.label(RichText::new("Container is gone").color(muted));
            return;
        };
        let running = c.state == "running";
        ui.horizontal(|ui| {
            for (t, name) in [
                (RightTab::Log, "Log"),
                (RightTab::Dashboard, "Dashboard"),
                (RightTab::Terminal, "Terminal"),
            ] {
                if ui.selectable_label(self.tab == t, name).clicked() {
                    self.tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Remove").clicked() {
                    cmds.push(Cmd::Remove(c.id.clone()));
                }
                if running {
                    if ui.button("Terminal").on_hover_text("Open a shell inside the container").clicked() {
                        self.tab = RightTab::Terminal;
                    }
                    if ui.button("Stop").clicked() {
                        cmds.push(Cmd::Stop(c.id.clone()));
                    }
                    if ui.button("Restart").clicked() {
                        cmds.push(Cmd::Restart(c.id.clone()));
                    }
                } else if ui.button("Start").clicked() {
                    cmds.push(Cmd::Start(c.id.clone()));
                }
            });
        });
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(dot.center(), 3.5, if running { theme.c(&theme.ui.success) } else { muted });
            ui.label(RichText::new(&c.name).strong());
            ui.label(
                RichText::new(c.id.chars().take(12).collect::<String>())
                    .monospace()
                    .color(theme.c(&theme.ui.warning)),
            );
            ui.label(RichText::new(&c.image).color(muted));
            if let Some(p) = &c.compose_project {
                ui.label(RichText::new(format!("Docker-compose: {p}")).color(theme.c(&theme.ui.accent)));
            }
            ui.label(RichText::new(&c.status).small().color(muted));
        });
        ui.separator();
        match self.tab {
            RightTab::Log => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.log_filter)
                            .hint_text("filter")
                            .desired_width(220.0),
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
            }
            RightTab::Dashboard => {
                egui::ScrollArea::both().id_salt("docker-dash").auto_shrink(false).show(ui, |ui| {
                    egui::Grid::new("docker-dash-grid")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            let base = [
                                ("Name", c.name.clone()),
                                ("Image", c.image.clone()),
                                ("Status", c.status.clone()),
                                ("Ports", c.ports.clone()),
                                ("Command", c.command.clone()),
                                ("Created", crate::git::cmd::fmt_date(c.created)),
                            ];
                            for (k, v) in base {
                                ui.label(RichText::new(k).color(muted));
                                ui.label(RichText::new(v).monospace());
                                ui.end_row();
                            }
                            for (k, v) in self.inspect.get(id).cloned().unwrap_or_default() {
                                if ["Name", "Image", "Status", "Command"].contains(&k.as_str()) {
                                    continue;
                                }
                                ui.label(RichText::new(k).color(muted));
                                ui.add(egui::Label::new(RichText::new(v).monospace()).wrap_mode(egui::TextWrapMode::Extend));
                                ui.end_row();
                            }
                        });
                });
            }
            RightTab::Terminal => {
                if !running {
                    ui.label(RichText::new("Container is not running").color(muted));
                    return;
                }
                let ctx = ui.ctx().clone();
                if !self.exec.contains_key(id) {
                    let cmd = [
                        "docker",
                        "exec",
                        "-it",
                        id,
                        "/bin/sh",
                        "-c",
                        "command -v bash >/dev/null && exec bash || exec sh",
                    ]
                    .map(String::from);
                    match Session::spawn(&self.root, Dims { cols: 100, rows: 24 }, ctx.clone(), Some(&cmd)) {
                        Ok(s) => {
                            self.exec.insert(id.to_owned(), s);
                        }
                        Err(e) => {
                            ui.label(RichText::new(format!("exec failed: {e:#}")).color(theme.c(&theme.ui.error)));
                            return;
                        }
                    }
                }
                let dead = self.exec.get(id).is_some_and(|s| s.exited);
                if dead && ui.button("Reconnect").clicked() {
                    self.exec.remove(id);
                    return;
                }
                if let Some(s) = self.exec.get_mut(id) {
                    s.tick();
                    crate::terminal::view::render(ui, s, theme, egui::Id::new("docker-exec").with(id));
                }
            }
        }
    }
}

/// One container line in the tree; returns the id when clicked.
fn container_row(ui: &mut egui::Ui, c: &ContainerRow, indent: f32, selected: bool, theme: &Theme, cmds: &mut Vec<Cmd>) -> Option<String> {
    let muted = theme.c(&theme.ui.fg_muted);
    let running = c.state == "running";
    let mut picked = None;
    let r = ui.horizontal(|ui| {
        ui.add_space(indent);
        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        if running {
            ui.painter().circle_filled(dot.center(), 3.5, theme.c(&theme.ui.success));
        } else {
            ui.painter().circle_stroke(dot.center(), 3.5, egui::Stroke::new(1.0, muted));
        }
        let r = ui.selectable_label(selected, RichText::new(&c.name).monospace());
        ui.label(RichText::new(&c.ports).small().color(muted));
        r
    });
    let r = r.inner;
    if r.clicked() {
        picked = Some(c.id.clone());
    }
    r.context_menu(|ui| {
        if running {
            if ui.button("Stop").clicked() {
                cmds.push(Cmd::Stop(c.id.clone()));
                ui.close();
            }
            if ui.button("Restart").clicked() {
                cmds.push(Cmd::Restart(c.id.clone()));
                ui.close();
            }
        } else if ui.button("Start").clicked() {
            cmds.push(Cmd::Start(c.id.clone()));
            ui.close();
        }
        if ui.button("Remove").clicked() {
            cmds.push(Cmd::Remove(c.id.clone()));
            ui.close();
        }
    });
    picked
}
