//! PTY plumbing: spawn the user's shell and pump bytes between it and the UI.

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
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

        Ok(Pty { master: pair.master, writer: input_tx, rx, wake_pending, child: Arc::new(Mutex::new(child)) })
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

#[cfg(test)]
mod tests {
    use super::*;

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
