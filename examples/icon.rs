//! Render the application icon: the letter ع on a dark rounded square.
//! Writes a PNG of the requested size (default 1024) to the path given.
use arabic_term::color::Rgb;
use arabic_term::font::Fonts;
use arabic_term::grid::Row;
use arabic_term::layout::{Layouter, RowContext};
use arabic_term::render::{Canvas, Target};
use arabic_term::term::BidiMode;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "icon.png".into());
    let size: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1024);

    let mut buf = vec![0u32; size * size];
    let mut canvas = Canvas { buf: &mut buf, width: size, height: size };
    // macOS icons are rounded squares inset from the canvas.
    let bg = Rgb(0x14, 0x16, 0x1b);
    let inset = size as f32 * 0.09;
    let side = size as f32 - 2.0 * inset;
    let radius = side * 0.225;
    canvas.clear(Rgb(0, 0, 0));
    // A solid rounded rect from horizontal strips.
    for y in 0..size {
        let fy = y as f32 + 0.5;
        if fy < inset || fy > inset + side {
            continue;
        }
        let dy = if fy < inset + radius {
            inset + radius - fy
        } else if fy > inset + side - radius {
            fy - (inset + side - radius)
        } else {
            0.0
        };
        let dx = (radius * radius - dy * dy).max(0.0).sqrt();
        let x0 = inset + radius - dx;
        let x1 = inset + side - radius + dx;
        canvas.fill_rect(x0, y as f32, x1 - x0, 1.0, bg);
    }
    // A prompt-coloured underline near the bottom, like a cursor bar.
    let accent = Rgb(0x62, 0xa0, 0xea);
    canvas.fill_rect(inset + side * 0.28, inset + side * 0.80, side * 0.44, side * 0.035, accent);

    // The letter, shaped through the same pipeline as the terminal's text.
    let px = size as f32 * 0.62;
    let mut fonts = Fonts::new(px, None)?;
    let row = Row { cells: vec![arabic_term::grid::Cell { ch: 'ع', ..Default::default() }], wrapped: false };
    let mut layouter = Layouter::new();
    let layout = layouter.layout_row_ctx(&row, 1, &mut fonts, BidiMode::Auto, true, RowContext { pre: "", post: "" });
    for g in &layout.glyphs {
        let Some(raster) = fonts.rasterize(g.font, g.glyph, g.px, 0.0) else { continue };
        let x = (size as f32 - raster.width as f32) / 2.0;
        let y = (size as f32 - raster.height as f32) / 2.0 - size as f32 * 0.03;
        canvas.glyph(raster, x as i32, y as i32, Rgb(0xe8, 0xed, 0xf5));
    }

    // Outside the rounded square must be transparent, so write RGBA.
    let mut rgba = Vec::with_capacity(size * size * 4);
    for px in buf.iter() {
        let (r, g, b) = ((px >> 16) as u8, (px >> 8) as u8, *px as u8);
        let a = if r == 0 && g == 0 && b == 0 { 0 } else { 255 };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    let file = std::fs::File::create(&out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size as u32, size as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({size}x{size})");
    Ok(())
}
