//! PTY plumbing: spawn the user's shell and pump bytes between it and the UI.

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::path::{Path, PathBuf};

fn pty_size(rows: u16, cols: u16, cell_w: u16, cell_h: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: cols.saturating_mul(cell_w), pixel_height: rows.saturating_mul(cell_h) }
}

/// Finder/Dock launches inherit `/`; use home instead of a surprising root shell.
pub fn start_directory(requested: Option<&Path>, current: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    requested.filter(|p| p.is_dir())
        .or_else(|| current.filter(|p| *p != Path::new("/") && p.is_dir()))
        .or_else(|| home.filter(|p| p.is_dir()))
        .or_else(|| current.filter(|p| p.is_dir()))
        .map(Path::to_path_buf)
}

/// Do not inherit an absent or unsupported C.UTF-8 locale on macOS. An
/// explicit, supported locale remains the user's choice, including LC_ALL=C.
pub fn configure_locale(cmd: &mut CommandBuilder) {
    let effective = ["LC_ALL", "LC_CTYPE", "LANG"].into_iter()
        .find_map(|key| cmd.get_env(key).and_then(|s| s.to_str())
            .filter(|s| !s.is_empty()).map(|s| (key, s.to_string())));
    let fallback = if cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" };
    match effective {
        Some((key, value)) if cfg!(target_os = "macos")
            && value.to_ascii_lowercase().replace('-', "") == "c.utf8" => { cmd.env(key, fallback); }
        None => { cmd.env("LC_CTYPE", fallback); }
        Some(("LANG", value)) if value == "C" || value == "POSIX" => { cmd.env("LC_CTYPE", fallback); }
        _ => {}
    }
}

#[derive(Default, Debug)]
pub struct DrainResult {
    pub bytes: usize,
    pub exited: bool,
    /// Budget exhausted; schedule another event-loop turn to drain the rest.
    pub yielded: bool,
}

fn drain_events(rx: &Receiver<PtyEvent>, mut output: impl FnMut(&[u8])) -> DrainResult {
    let start = Instant::now();
    let mut result = DrainResult::default();
    while let Ok(event) = rx.try_recv() {
        match event {
            PtyEvent::Output(bytes) => {
                output(&bytes);
                result.bytes += bytes.len();
            }
            PtyEvent::Exited => result.exited = true,
        }
        if result.bytes >= 128 * 1024 || start.elapsed() >= Duration::from_millis(2) {
            result.yielded = true;
            break;
        }
    }
    result
}

pub enum PtyEvent {
    /// A chunk of output from the child.
    Output(Vec<u8>),
    /// The child exited; the session is over.
    Exited,
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Sender<Vec<u8>>,
    pub rx: Receiver<PtyEvent>,
    wake_pending: Arc<AtomicBool>,
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
}

impl Pty {
    /// Spawn a shell on a new PTY. `program` defaults to `$SHELL` started as a
    /// login shell. `wake` is coalesced until `begin_drain` acknowledges it,
    /// so output bursts cannot flood the window event queue.
    pub fn spawn(
        program: Option<&str>,
        rows: u16,
        cols: u16,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<&str>,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Pty> {
        let sys = native_pty_system();
        let pair = sys.openpty(pty_size(rows, cols, cell_w, cell_h))?;

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
        configure_locale(&mut cmd);
        // Start where asked (a new window inherits its parent's directory),
        // else where we were started.
        let here = std::env::current_dir().ok();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(dir) = start_directory(cwd.map(Path::new), here.as_deref(), home.as_deref()) {
            log::debug!("starting shell {shell} in {}", dir.display());
            cmd.cwd(dir);
        }

        let child = pair.slave.spawn_command(cmd)?;
        // The slave fd must be dropped here, or the master never sees EOF when
        // the child exits and the reader thread blocks forever.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let (input_tx, input_rx) = channel::<Vec<u8>>();
        // A child can stop reading while producing output. Never block the
        // UI on its input pipe, especially now that output has backpressure:
        // the UI must remain free to drain output and handle the next key.
        std::thread::Builder::new().name("pty-writer".into()).spawn(move || {
            while let Ok(bytes) = input_rx.recv() {
                if let Err(e) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                    log::warn!("pty write failed: {e}");
                    break;
                }
            }
        })?;
        // Backpressure bounds queued output to 1 MiB. Smaller chunks also
        // bound how much parsing can overrun the UI's per-turn time budget.
        let (tx, rx) = sync_channel(64);
        let wake_pending = Arc::new(AtomicBool::new(false));
        let reader_wake = wake_pending.clone();

        std::thread::Builder::new().name("pty-reader".into()).spawn(move || {
            let mut buf = vec![0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(PtyEvent::Exited);
                        if !reader_wake.swap(true, Ordering::AcqRel) {
                            wake();
                        }
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                        if !reader_wake.swap(true, Ordering::AcqRel) {
                            wake();
                        }
                    }
                }
            }
        })?;

        Ok(Pty { master: pair.master, writer: input_tx, rx, wake_pending, child: Mutex::new(Some(child)) })
    }

    /// Acknowledge before draining: output arriving after this point queues a
    /// new wake, including when it races with the final empty receive.
    pub fn begin_drain(&self) {
        self.wake_pending.store(false, Ordering::Release);
    }

    /// Parse directly from queued chunks, yielding to input after 2 ms or
    /// 128 KiB. Callers must schedule another turn when `yielded` is true.
    pub fn drain(&self, output: impl FnMut(&[u8])) -> DrainResult {
        self.begin_drain();
        drain_events(&self.rx, output)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.writer.send(bytes.to_vec()).is_err() {
            log::debug!("pty writer has exited");
        }
    }

    pub fn resize(&self, rows: u16, cols: u16, cell_w: u16, cell_h: u16) {
        if let Err(e) = self.master.resize(pty_size(rows, cols, cell_w, cell_h)) {
            log::warn!("could not resize the PTY: {e}");
        }
    }

    pub fn kill(&self) {
        if let Some(mut child) = self.child.lock().ok().and_then(|mut child| child.take()) {
            // Child::kill may wait for a grace period; keep that off the UI
            // thread. Always wait afterwards so closed windows leave no zombies.
            std::thread::spawn(move || {
                let _ = child.kill();
                if let Err(e) = child.wait() { log::debug!("waiting for shell exit: {e}"); }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_font_pixel_dimensions_saturate_instead_of_overflowing() {
        let size = pty_size(1000, 1000, 200, 250);
        assert_eq!((size.rows, size.cols), (1000, 1000));
        assert_eq!((size.pixel_width, size.pixel_height), (u16::MAX, u16::MAX));
    }

    #[test]
    fn output_flood_yields_and_resumes_without_losing_bytes_or_exit() {
        let (tx, rx) = sync_channel(64);
        for n in 0..32u8 {
            tx.send(PtyEvent::Output(vec![n; 16384])).unwrap();
        }
        tx.send(PtyEvent::Exited).unwrap();
        let mut received = Vec::new();
        let first = drain_events(&rx, |b| received.extend_from_slice(b));
        assert!(first.yielded);
        assert!(first.bytes <= 128 * 1024);
        assert!(!first.exited);
        let mut exited = false;
        loop {
            let next = drain_events(&rx, |b| received.extend_from_slice(b));
            exited |= next.exited;
            if !next.yielded {
                assert!(exited);
                break;
            }
        }
        let expected: Vec<_> = (0..32u8).flat_map(|n| std::iter::repeat(n).take(16384)).collect();
        assert_eq!(received, expected);
    }

    #[test]
    fn slow_parse_yields_even_before_byte_limit() {
        let (tx, rx) = sync_channel(2);
        tx.send(PtyEvent::Output(vec![1])).unwrap();
        tx.send(PtyEvent::Output(vec![2])).unwrap();
        let result = drain_events(&rx, |_| std::thread::sleep(Duration::from_millis(3)));
        assert!(result.yielded);
        assert_eq!(result.bytes, 1);
        assert!(matches!(rx.try_recv(), Ok(PtyEvent::Output(b)) if b == [2]));
    }
}

impl Drop for Pty {
    /// Closing a window ends its shell; nothing should outlive the tab.
    fn drop(&mut self) {
        self.kill();
    }
}
