//! Write the application icon at a requested size.
//!
//! The icon is drawn once, in `assets/icon.svg`, and kept rendered at 1024 px
//! in `assets/icon.png`. This scales that image down by averaging the source
//! pixels each output pixel covers, so the packaging scripts need no image
//! tools beyond the ones the build already has.
//!
//! Usage: `icon <out.png> [size]` (default 1024).

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "icon.png".into());
    let size: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1024);
    anyhow::ensure!(size > 0, "size must be positive");

    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png");
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(source)?)).read_info()?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    anyhow::ensure!(
        reader.info().color_type == png::ColorType::Rgba && reader.info().bit_depth == png::BitDepth::Eight,
        "{source}: expected 8-bit RGBA"
    );
    let mut src = vec![0u8; w * h * 4];
    reader.next_frame(&mut src)?;

    // Area-average each output pixel over the source rectangle it covers,
    // weighting colour by alpha so the transparent corners do not darken the
    // rounded edge.
    let mut rgba = Vec::with_capacity(size * size * 4);
    let (sx, sy) = (w as f64 / size as f64, h as f64 / size as f64);
    for oy in 0..size {
        let (y0, y1) = (oy as f64 * sy, (oy + 1) as f64 * sy);
        for ox in 0..size {
            let (x0, x1) = (ox as f64 * sx, (ox + 1) as f64 * sx);
            let (mut acc, mut alpha, mut area) = ([0.0f64; 3], 0.0f64, 0.0f64);
            for y in y0.floor() as usize..(y1.ceil() as usize).min(h) {
                let wy = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).max(0.0);
                for x in x0.floor() as usize..(x1.ceil() as usize).min(w) {
                    let wx = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).max(0.0);
                    let p = &src[(y * w + x) * 4..][..4];
                    let weight = wx * wy;
                    let a = p[3] as f64 / 255.0 * weight;
                    for c in 0..3 {
                        acc[c] += p[c] as f64 * a;
                    }
                    alpha += a;
                    area += weight;
                }
            }
            for c in acc {
                rgba.push(if alpha > 0.0 { (c / alpha).round() as u8 } else { 0 });
            }
            rgba.push(if area > 0.0 { (alpha / area * 255.0).round() as u8 } else { 0 });
        }
    }

    let file = std::fs::File::create(&out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size as u32, size as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({size}x{size})");
    Ok(())
}
