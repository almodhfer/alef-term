//! Frame-time benchmark for the render path.
use arabic_term::color::Palette;
use arabic_term::font::Fonts;
use arabic_term::render::{Canvas, Renderer};
use arabic_term::term::{Terminal, Vt};
use std::time::Instant;

fn bench_at(name: &str, size: f32, rows: usize, cols: usize, gen: impl Fn(usize) -> String) {
    let mut fonts = Fonts::new(size, None).unwrap();
    let mut term = Terminal::new(rows, cols, 1000);
    let mut vt = Vt::new();
    for r in 0..rows {
        vt.feed(&mut term, format!("\x1b[{};1H", r + 1).as_bytes());
        vt.feed(&mut term, gen(r).as_bytes());
    }
    let w = (cols as f32 * fonts.cell_w) as usize + 8;
    let h = (rows as f32 * fonts.cell_h) as usize + 4;
    let mut buf = vec![0u32; w * h];
    let mut renderer = Renderer::new();
    let pal = Palette::dark();

    // warm the glyph cache
    for _ in 0..3 {
        let mut c = Canvas { buf: &mut buf, width: w, height: h };
        renderer.draw(&mut c, &term, &mut fonts, &pal, None, true);
    }
    let n = 60;
    let t0 = Instant::now();
    for _ in 0..n {
        let mut c = Canvas { buf: &mut buf, width: w, height: h };
        renderer.draw(&mut c, &term, &mut fonts, &pal, None, true);
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("{name:28} {rows}x{cols} @ {w}x{h}px  {per:6.2} ms/frame  ({:5.0} fps)", 1000.0 / per);
}

fn bench(name: &str, rows: usize, cols: usize, gen: impl Fn(usize) -> String) {
    bench_at(name, 15.0, rows, cols, gen)
}

fn main() {
    bench("ascii", 50, 200, |r| format!("row {r} the quick brown fox jumps over the lazy dog 0123456789"));
    bench("arabic", 50, 200, |_| "مرحبا بالعالم من الطرفية العربية التي تدعم النص العربي".into());
    bench("mixed", 50, 200, |r| format!("$ ls ~/مشاريع/{r} && echo \"تم بنجاح\" # ok"));
    bench("arabic+tashkeel", 50, 200, |_| "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ".into());
    println!("--- retina (2x device pixels) ---");
    bench_at("ascii @2x", 30.0, 50, 200, |r| format!("row {r} the quick brown fox jumps over the lazy dog 0123456789"));
    bench_at("arabic @2x", 30.0, 50, 200, |_| "مرحبا بالعالم من الطرفية العربية التي تدعم النص العربي".into());
}
