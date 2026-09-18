//! Compare installed Arabic faces using the actual terminal shaping and renderer.
use arabic_term::{color::Palette, font::{ArabicFont, Fonts}, render::{Canvas, DrawOpts, Renderer}, term::{Terminal, Vt}};

fn main() -> anyhow::Result<()> {
    let out = std::env::var("OUT_DIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    std::fs::create_dir_all(&out)?;
    for (name, choice) in [
        ("naskh", ArabicFont::Naskh), ("amiri", ArabicFont::Amiri),
        ("scheherazade", ArabicFont::Scheherazade), ("geeza", ArabicFont::Geeza),
    ] {
        let mut fonts = Fonts::with_families(32.0, None, None, Some(choice.family()))?;
        if fonts.arabic_family != choice.family() { continue; }
        let mut term = Terminal::new(7, 68, 0);
        term.set_palette(Palette::light());
        let text = format!(concat!(
            "\x1b[1m{}\x1b[0m   |   16 pt · Retina\r\n",
            "جمالُ اللُّغةِ العربيَّةِ في تفاصيلِ حُروفِها\r\n",
            "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ\r\n",
            "افتح الملف README ثم احفظ التغييرات\r\n",
            "\x1b[1mنص عريض: العربية جميلة وواضحة\x1b[0m\r\n",
            "السطر ٤٢ من 128 — مشروع terminal\r\n",
        ), choice.family());
        Vt::new().feed(&mut term, text.as_bytes());
        let mut renderer = Renderer::new();
        renderer.pad_x = 24.0;
        renderer.pad_y = 20.0;
        renderer.minimum_contrast = 4.5;
        let w = (term.grid.cols as f32 * fonts.cell_w + 48.0) as usize;
        let h = (term.grid.rows as f32 * fonts.cell_h + 40.0) as usize;
        let mut pixels = vec![0; w*h];
        renderer.draw(&mut Canvas { buf: &mut pixels, width: w, height: h }, &term,
            &mut fonts, &DrawOpts { cursor_on: false, ..Default::default() });
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        for px in pixels { ppm.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8]); }
        std::fs::write(std::path::Path::new(&out).join(format!("arabic-{name}.ppm")), ppm)?;
        println!("{}: cell {}x{}, baseline {}, Arabic scale {:.2}", choice.family(), fonts.cell_w, fonts.cell_h, fonts.baseline, fonts.arabic_scale);
    }
    Ok(())
}
