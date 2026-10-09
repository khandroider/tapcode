//! Docker engine access on a background tokio runtime; UI talks over channels.

use bollard::Docker;
use bollard::query_parameters::{
    CreateImageOptionsBuilder, ListContainersOptionsBuilder, ListImagesOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    RemoveImageOptionsBuilder, RestartContainerOptionsBuilder, StartContainerOptions, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
}

#[derive(Clone, Debug)]
pub struct ImageRow {
    pub id: String,
    pub tags: String,
    pub size: i64,
    pub created: i64,
}

pub enum Cmd {
    Refresh,
    Start(String),
    Stop(String),
    Restart(String),
    Remove(String),
    RemoveImage(String),
    Pull(String),
    /// Stream logs of a container (tail 500, follow) until another Logs request replaces it.
    Logs(String),
    Compose {
        file: PathBuf,
        args: Vec<String>,
    },
}

pub enum Event {
    Containers(Vec<ContainerRow>),
    Images(Vec<ImageRow>),
    Log(String),
    LogsCleared,
    Console(String),
    Error(String),
    Done,
}

pub struct DockerApi {
    tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
}

impl DockerApi {
    pub fn start(root: PathBuf, ctx: egui::Context) -> Self {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (ev_tx, rx) = channel::<Event>();
        std::thread::Builder::new()
            .name("docker".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ev_tx.send(Event::Error(format!("tokio: {e}")));
                        return;
                    }
                };
                rt.block_on(run(root, cmd_rx, ev_tx, ctx));
            })
            .expect("spawn docker thread");
        Self { tx, rx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

async fn run(root: PathBuf, rx: Receiver<Cmd>, tx: Sender<Event>, ctx: egui::Context) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(Event::Error(format!("docker: {e}")));
            return;
        }
    };
    if let Err(e) = docker.version().await {
        let _ = tx.send(Event::Error(format!("docker daemon unreachable: {e}")));
        ctx.request_repaint();
        return;
    }
    let logs_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    // Commands arrive on a std channel; poll it without blocking the runtime.
    loop {
        let cmd = match rx.try_recv() {
            Ok(c) => c,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => return,
        };
        let r: Result<(), bollard::errors::Error> = match cmd {
            Cmd::Refresh => refresh(&docker, &tx).await,
            Cmd::Start(id) => docker.start_container(&id, None::<StartContainerOptions>).await.map(|_| ()),
            Cmd::Stop(id) => {
                docker
                    .stop_container(&id, Some(StopContainerOptionsBuilder::default().t(10).build()))
                    .await
            }
            Cmd::Restart(id) => {
                docker
                    .restart_container(&id, Some(RestartContainerOptionsBuilder::default().t(10).build()))
                    .await
            }
            Cmd::Remove(id) => {
                docker
                    .remove_container(&id, Some(RemoveContainerOptionsBuilder::default().force(true).build()))
                    .await
            }
            Cmd::RemoveImage(id) => docker
                .remove_image(&id, Some(RemoveImageOptionsBuilder::default().force(false).build()), None)
                .await
                .map(|_| ()),
            Cmd::Pull(name) => {
                let (image, tag) = name
                    .rsplit_once(':')
                    .filter(|(i, _)| !i.contains('/') || !i.ends_with('/'))
                    .unwrap_or((name.as_str(), "latest"));
                let opts = CreateImageOptionsBuilder::default().from_image(image).tag(tag).build();
                let mut stream = docker.create_image(Some(opts), None, None);
                let mut last = String::new();
                let mut err = None;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(info) => {
                            let s = format!("{} {}", info.status.unwrap_or_default(), info.id.unwrap_or_default());
                            if s != last {
                                let _ = tx.send(Event::Console(format!("pull {name}: {s}")));
                                ctx.request_repaint();
                                last = s;
                            }
                        }
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                match err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
            Cmd::Logs(id) => {
                if let Some(h) = logs_task.lock().unwrap().take() {
                    h.abort();
                }
                let _ = tx.send(Event::LogsCleared);
                let d = docker.clone();
                let t = tx.clone();
                let c = ctx.clone();
                let handle = tokio::spawn(async move {
                    let opts = LogsOptionsBuilder::default()
                        .follow(true)
                        .stdout(true)
                        .stderr(true)
                        .tail("500")
                        .timestamps(false)
                        .build();
                    let mut stream = d.logs(&id, Some(opts));
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(out) => {
                                let s = String::from_utf8_lossy(&out.into_bytes()).into_owned();
                                for line in s.lines() {
                                    let _ = t.send(Event::Log(line.to_owned()));
                                }
                                c.request_repaint();
                            }
                            Err(e) => {
                                let _ = t.send(Event::Error(format!("logs: {e}")));
                                break;
                            }
                        }
                    }
                });
                *logs_task.lock().unwrap() = Some(handle);
                Ok(())
            }
            Cmd::Compose { file, args } => {
                let _ = tx.send(Event::Console(format!("$ docker compose -f {} {}", file.display(), args.join(" "))));
                let (f, a, r) = (file.clone(), args.clone(), root.clone());
                let out = tokio::task::spawn_blocking(move || {
                    std::process::Command::new("docker")
                        .arg("compose")
                        .arg("-f")
                        .arg(&f)
                        .args(&a)
                        .current_dir(&r)
                        .output()
                })
                .await
                .map_err(std::io::Error::other)
                .and_then(|r| r);
                match out {
                    Ok(o) => {
                        for line in String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .chain(String::from_utf8_lossy(&o.stderr).lines())
                        {
                            if !line.trim().is_empty() {
                                let _ = tx.send(Event::Console(line.to_owned()));
                            }
                        }
                        Ok(())
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Error(format!("compose: {e}")));
                        Ok(())
                    }
                }
            }
        };
        if let Err(e) = r {
            let _ = tx.send(Event::Error(e.to_string()));
        }
        let _ = tx.send(Event::Done);
        ctx.request_repaint();
    }
}

async fn refresh(docker: &Docker, tx: &Sender<Event>) -> Result<(), bollard::errors::Error> {
    let containers = docker
        .list_containers(Some(ListContainersOptionsBuilder::default().all(true).build()))
        .await?;
    let rows = containers
        .into_iter()
        .map(|c| {
            let mut ports: Vec<String> = c
                .ports
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.public_port.map(|pub_| format!("{pub_}→{}", p.private_port)))
                .collect();
            ports.sort();
            ports.dedup();
            ContainerRow {
                id: c.id.unwrap_or_default(),
                name: c
                    .names
                    .unwrap_or_default()
                    .first()
                    .map(|n| n.trim_start_matches('/').to_owned())
                    .unwrap_or_default(),
                image: c.image.unwrap_or_default(),
                state: c.state.map(|s| format!("{s:?}").to_lowercase()).unwrap_or_default(),
                status: c.status.unwrap_or_default(),
                ports: ports.join(", "),
            }
        })
        .collect();
    let _ = tx.send(Event::Containers(rows));
    let images = docker
        .list_images(Some(ListImagesOptionsBuilder::default().all(false).build()))
        .await?;
    let rows = images
        .into_iter()
        .map(|i| ImageRow {
            id: i.id.clone(),
            tags: if i.repo_tags.is_empty() {
                "<none>".into()
            } else {
                i.repo_tags.join(", ")
            },
            size: i.size,
            created: i.created,
        })
        .collect();
    let _ = tx.send(Event::Images(rows));
    Ok(())
}

/// Compose files in the project root (JetBrains lists these under Services).
pub fn compose_files(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    (n.starts_with("docker-compose") || n.starts_with("compose")) && (n.ends_with(".yml") || n.ends_with(".yaml"))
                })
                .collect()
        })
        .unwrap_or_default();
    for sub in ["dev", "docker"] {
        let d = root.join(sub);
        if d.is_dir() {
            out.extend(compose_files(&d));
        }
    }
    out.sort();
    out
}

pub fn human_size(b: i64) -> String {
    let f = b as f64;
    if f > 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f > 1e6 {
        format!("{:.0} MB", f / 1e6)
    } else {
        format!("{:.0} KB", f / 1e3)
    }
}
