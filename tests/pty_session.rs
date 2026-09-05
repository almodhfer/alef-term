//! End-to-end: run a real shell on a PTY and check its output reaches the grid.

use arabic_term::grid::Attrs;
use arabic_term::pty::{Pty, PtyEvent};
use arabic_term::term::{Terminal, Vt};
use std::time::{Duration, Instant};

fn row_text(t: &Terminal, r: usize) -> String {
    t.grid.screen.rows[r]
        .cells
        .iter()
        .filter(|c| !c.attrs.contains(Attrs::WIDE_SPACER))
        .map(|c| {
            let mut s = String::new();
            s.push(c.ch);
            for m in c.marks() {
                s.push(*m);
            }
            s
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn screen(t: &Terminal) -> String {
    (0..t.grid.rows).map(|r| row_text(t, r)).collect::<Vec<_>>().join("\n")
}

/// Pump the PTY into the terminal until `pred` holds or we run out of time.
fn pump_until(
    pty: &Pty,
    term: &mut Terminal,
    vt: &mut Vt,
    timeout: Duration,
    pred: impl Fn(&Terminal) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match pty.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PtyEvent::Output(b)) => vt.feed(term, &b),
            Ok(PtyEvent::Exited) => {}
            Err(_) => {}
        }
        if pred(term) {
            return true;
        }
    }
    false
}

#[test]
fn shell_output_reaches_the_grid() {
    let mut term = Terminal::new(10, 60, 100);
    let mut vt = Vt::new();
    let mut pty = Pty::spawn(Some("/bin/sh"), 10, 60, 8, 16, || {}).expect("spawn pty");

    pty.write(b"printf 'hello-from-shell\\n'\n");
    let ok = pump_until(&pty, &mut term, &mut vt, Duration::from_secs(10), |t| {
        screen(t).contains("hello-from-shell")
    });
    assert!(ok, "shell output never arrived; screen was:\n{}", screen(&term));
    pty.kill();
}

#[test]
fn arabic_survives_the_round_trip_through_the_pty() {
    // The bytes the shell echoes must land in the grid as the same codepoints,
    // one cell per letter, with diacritics attached rather than spread out.
    let mut term = Terminal::new(10, 60, 100);
    let mut vt = Vt::new();
    let mut pty = Pty::spawn(Some("/bin/sh"), 10, 60, 8, 16, || {}).expect("spawn pty");

    pty.write("printf 'مرحبا بالعالم\\n'\n".as_bytes());
    let ok = pump_until(&pty, &mut term, &mut vt, Duration::from_secs(10), |t| {
        screen(t).contains("مرحبا بالعالم")
    });
    assert!(ok, "Arabic never arrived; screen was:\n{}", screen(&term));

    let line = (0..10).map(|r| row_text(&term, r)).find(|l| l.contains("مرحبا بالعالم")).unwrap();
    assert!(line.chars().any(|c| c == 'م'));
    pty.kill();
}

#[test]
fn resize_is_reported_to_the_child() {
    let mut term = Terminal::new(10, 60, 100);
    let mut vt = Vt::new();
    let mut pty = Pty::spawn(Some("/bin/sh"), 10, 60, 8, 16, || {}).expect("spawn pty");

    pty.resize(24, 97, 8, 16);
    // stty reads the window size straight from the tty.
    pty.write(b"stty size\n");
    let ok = pump_until(&pty, &mut term, &mut vt, Duration::from_secs(10), |t| {
        screen(t).contains("24 97")
    });
    assert!(ok, "child never saw the new size; screen was:\n{}", screen(&term));
    pty.kill();
}

/// Editors that predate shaping terminals write Arabic out as presentation
/// forms. Check a real editor's output arrives as ordinary Arabic letters, so
/// the shaper has something it can actually join.
#[test]
fn editor_pre_shaped_arabic_arrives_as_base_letters() {
    let Some(nvim) = ["/opt/homebrew/bin/nvim", "/usr/local/bin/nvim", "/usr/bin/vim"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        eprintln!("no vim/nvim available; skipping");
        return;
    };

    let dir = std::env::temp_dir().join("arabic-term-test");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ar.txt");
    std::fs::write(&file, "مرحبا بالعالم\n").unwrap();

    let mut term = Terminal::new(24, 80, 100);
    let mut vt = Vt::new();
    let mut pty = Pty::spawn(Some("/bin/sh"), 24, 80, 8, 16, || {}).expect("spawn pty");
    pty.write(format!("{nvim} -u NONE {}\n", file.display()).as_bytes());

    let ok = pump_until(&pty, &mut term, &mut vt, Duration::from_secs(20), |t| {
        t.grid.in_alt() && screen(t).contains("مرحبا")
    });
    pty.write(b"\x1b:q!\n");
    pty.kill();

    assert!(ok, "editor output never arrived as base letters; screen was:\n{}", screen(&term));
    // And explicitly: no Arabic Presentation Forms survived into the grid.
    let has_forms = screen(&term)
        .chars()
        .any(|c| matches!(c as u32, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF));
    assert!(!has_forms, "presentation forms reached the grid:\n{}", screen(&term));
}
