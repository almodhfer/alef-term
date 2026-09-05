//! PTY plumbing: spawn the user's shell and pump bytes between it and the UI.

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub enum PtyEvent {
    /// A chunk of output from the child.
    Output(Vec<u8>),
    /// The child exited; the session is over.
    Exited,
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pub rx: Receiver<PtyEvent>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
}

impl Pty {
    /// Spawn a shell on a new PTY. `program` defaults to `$SHELL` started as a
    /// login shell. `wake` is called after each chunk is queued, so the UI
    /// thread can be nudged without polling.
    pub fn spawn(
        program: Option<&str>,
        rows: u16,
        cols: u16,
        cell_w: u16,
        cell_h: u16,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Pty> {
        let sys = native_pty_system();
        let pair = sys.openpty(PtySize {
            rows,
            cols,
            pixel_width: cols * cell_w,
            pixel_height: rows * cell_h,
        })?;

        let login = program.is_none();
        let shell = match program {
            Some(p) => p.to_string(),
            None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
        };
        let mut cmd = CommandBuilder::new(&shell);
        if login {
            cmd.arg("-l");
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "arabic-term");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd)?;
        // The slave fd must be dropped here, or the master never sees EOF when
        // the child exits and the reader thread blocks forever.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, rx): (Sender<PtyEvent>, Receiver<PtyEvent>) = channel();

        std::thread::Builder::new().name("pty-reader".into()).spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(PtyEvent::Exited);
                        wake();
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                        wake();
                    }
                }
            }
        })?;

        Ok(Pty { master: pair.master, writer, rx, child: Arc::new(Mutex::new(child)) })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if let Err(e) = self.writer.write_all(bytes) {
            log::warn!("pty write failed: {e}");
            return;
        }
        let _ = self.writer.flush();
    }

    pub fn resize(&self, rows: u16, cols: u16, cell_w: u16, cell_h: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: cols * cell_w,
            pixel_height: rows * cell_h,
        });
    }

    pub fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }
}
