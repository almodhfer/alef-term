//! Render a sixel image plus Arabic text offline, to check images land right.
use alef_term::font::Fonts;
use alef_term::render::{Canvas, DrawOpts, Renderer};
use alef_term::term::{Terminal, Vt};

fn main() -> anyhow::Result<()> {
    let (rows, cols) = (16usize, 50usize);
    let mut fonts = Fonts::new(16.0, None)?;
    let mut term = Terminal::new(rows, cols, 500);
    term.set_cell_px(fonts.cell_w, fonts.cell_h);
    let mut vt = Vt::new();

    // Build a 60x36 sixel: six bands of colour blocks.
    let mut sx = String::from("\x1bPq\"1;1;60;36");
    let colors = [(100,20,20),(20,100,20),(20,20,100),(100,100,20),(100,20,100),(20,100,100)];
    for (i, (r, g, b)) in colors.iter().enumerate() {
        sx.push_str(&format!("#{};2;{};{};{}", i + 1, r, g, b));
        sx.push_str(&format!("!60~"));
        sx.push('-');
    }
    sx.push_str("\x1b\\");

    vt.feed(&mut term, b"\x1b[1;32m$\x1b[0m img2sixel \xd8\xb5\xd9\x88\xd8\xb1\xd8\xa9.png\r\n");
    vt.feed(&mut term, sx.as_bytes());
    vt.feed(&mut term, "مرحبا — الصورة أعلاه\r\n".as_bytes());
    vt.feed(&mut term, b"\x1b[1;32m$\x1b[0m ");

    println!("images placed: {}", term.images.len());
    for p in &term.images {
        println!("  line={} col={} {}x{}", p.line, p.col, p.image.width, p.image.height);
    }

    let mut renderer = Renderer::new();
    let w = (cols as f32 * fonts.cell_w) as usize + 8;
    let h = (rows as f32 * fonts.cell_h) as usize + 4;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    renderer.draw(&mut canvas, &term, &mut fonts, &DrawOpts::default());

    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in buf.iter() {
        ppm.push((px >> 16) as u8); ppm.push((px >> 8) as u8); ppm.push(*px as u8);
    }
    let out = std::env::var("OUT").unwrap_or("/tmp/sixel.ppm".into());
    std::fs::write(&out, ppm)?;
    println!("wrote {out} {w}x{h}");
    Ok(())
}
