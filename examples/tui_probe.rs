//! Drive a real full-screen program in a PTY and render what it produces.
use arabic_term::color::Palette;
use arabic_term::font::Fonts;
use arabic_term::pty::{Pty, PtyEvent};
use arabic_term::render::{Canvas, Renderer};
use arabic_term::term::{Terminal, Vt};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prog = args.first().cloned().unwrap_or("/bin/sh".into());
    let script = args.get(1).cloned().unwrap_or_default();
    let settle_ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2500);
    let out = std::env::var("OUT").unwrap_or("/tmp/tui.ppm".into());

    let (rows, cols) = (24usize, 80usize);
    let mut fonts = Fonts::new(16.0, None)?;
    let mut term = Terminal::new(rows, cols, 500);
    let mut vt = Vt::new();
    let mut pty = Pty::spawn(Some(&prog), rows as u16, cols as u16, 8, 16, || {})?;

    // Send the driving keystrokes, letting the program settle between them.
    for chunk in script.split("<<PAUSE>>") {
        if !chunk.is_empty() {
            pty.write(chunk.as_bytes());
        }
        let deadline = Instant::now() + Duration::from_millis(settle_ms);
        while Instant::now() < deadline {
            match pty.rx.recv_timeout(Duration::from_millis(120)) {
                Ok(PtyEvent::Output(b)) => vt.feed(&mut term, &b),
                Ok(PtyEvent::Exited) => break,
                Err(_) => {}
            }
        }
    }
    pty.kill();

    println!("alt_screen={} title={:?} cursor=({},{}) visible={}", term.grid.in_alt(), term.title,
        term.grid.screen.cursor.row, term.grid.screen.cursor.col,
        term.modes.contains(arabic_term::term::Modes::CURSOR_VISIBLE));
    use arabic_term::term::Modes;
    println!("mouse: click(1000)={} drag(1002)={} motion(1003)={} sgr(1006)={} => grabbed={}",
        term.modes.contains(Modes::MOUSE_CLICK),
        term.modes.contains(Modes::MOUSE_DRAG),
        term.modes.contains(Modes::MOUSE_MOTION),
        term.modes.contains(Modes::MOUSE_SGR),
        term.modes.intersects(Modes::MOUSE_CLICK | Modes::MOUSE_DRAG | Modes::MOUSE_MOTION));
    for r in 0..rows {
        let line: String = term.grid.screen.rows[r]
            .cells
            .iter()
            .filter(|c| !c.attrs.contains(arabic_term::grid::Attrs::WIDE_SPACER))
            .map(|c| c.ch)
            .collect();
        println!("{:2}|{}|", r, line.trim_end());
    }

    let mut renderer = Renderer::new();
    let w = (cols as f32 * fonts.cell_w) as usize + 8;
    let h = (rows as f32 * fonts.cell_h) as usize + 4;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    renderer.draw(&mut canvas, &term, &mut fonts, &Palette::dark(), None, true);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in buf.iter() {
        ppm.push((px >> 16) as u8);
        ppm.push((px >> 8) as u8);
        ppm.push(*px as u8);
    }
    std::fs::write(&out, ppm)?;
    println!("wrote {out} {w}x{h}");
    Ok(())
}
