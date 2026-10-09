//! One terminal session: PTY + shell process + alacritty terminal state machine.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Clone, Copy)]
pub struct Dims {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Proxy(Sender<Event>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

pub struct Session {
    pub term: Term<Proxy>,
    parser: Processor,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    bytes_rx: Receiver<Vec<u8>>,
    events_rx: Receiver<Event>,
    pub title: String,
    pub dims: Dims,
    pub exited: bool,
}

impl Session {
    pub fn spawn(cwd: &Path, dims: Dims, ctx: egui::Context, command: Option<&[String]>) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize {
            rows: dims.rows as u16,
            cols: dims.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let (mut cmd, title) = match command {
            Some([prog, args @ ..]) => {
                let mut c = CommandBuilder::new(prog);
                c.args(args);
                (
                    c,
                    args.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_else(|| prog.clone()),
                )
            }
            _ => {
                let mut c = CommandBuilder::new(&shell);
                c.arg("-l");
                (c, shell.rsplit('/').next().unwrap_or("shell").to_owned())
            }
        };
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "code-editor");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (bytes_tx, bytes_rx) = channel();
        std::thread::Builder::new().name("pty-reader".into()).spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if bytes_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
            }
            ctx.request_repaint();
        })?;
        let (ev_tx, events_rx) = channel();
        let term = Term::new(Config::default(), &dims, Proxy(ev_tx));
        Ok(Self {
            term,
            parser: Processor::new(),
            master: pair.master,
            writer,
            child,
            bytes_rx,
            events_rx,
            title,
            dims,
            exited: false,
        })
    }

    /// Feed pending PTY output into the terminal; answer terminal queries.
    pub fn tick(&mut self) {
        while let Ok(bytes) = self.bytes_rx.try_recv() {
            self.parser.advance(&mut self.term, &bytes);
        }
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                Event::PtyWrite(s) => self.write(s.as_bytes()),
                Event::Title(t) => self.title = t,
                Event::ResetTitle => self.title = "shell".into(),
                _ => {}
            }
        }
        if !self.exited && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.exited = true;
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if !self.exited {
            let _ = self.writer.write_all(bytes);
            let _ = self.writer.flush();
        }
    }

    pub fn resize(&mut self, dims: Dims) {
        if dims.cols == self.dims.cols && dims.rows == self.dims.rows || dims.cols < 2 || dims.rows < 1 {
            return;
        }
        self.dims = dims;
        let _ = self.master.resize(PtySize {
            rows: dims.rows as u16,
            cols: dims.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.term.resize(dims);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
