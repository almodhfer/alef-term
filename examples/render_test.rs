//! Render a showcase screen offline to a PPM, without opening a window.
//! Useful for eyeballing layout changes and for regression screenshots.
use arabic_term::font::Fonts;
use arabic_term::render::{Canvas, DrawOpts, Renderer};
use arabic_term::term::{BidiMode, Terminal, Vt};

const SCRIPT: &str = concat!(
    "\x1b[1;36m~/مشاريع\x1b[0m \x1b[1;32m$\x1b[0m echo \"مرحبا بالعالم\"\r\n",
    "مرحبا بالعالم\r\n",
    "\x1b[1;32m$\x1b[0m cat بسملة.txt\r\n",
    "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ\r\n",
    "\x1b[1;32m$\x1b[0m ls\r\n",
    "\x1b[1;34mملفاتي\x1b[0m    \x1b[1;34msrc\x1b[0m       Cargo.toml   README.md\r\n",
    "تقرير.pdf  صورة.png  بسملة.txt    main.rs\r\n",
    "\x1b[1;32m$\x1b[0m git commit -m \"إصلاح خطأ في الترميز\"\r\n",
    "\x1b[32m[main a3f9c21]\x1b[0m إصلاح خطأ في الترميز\r\n",
    " 1 file changed, \x1b[32m42 insertions(+)\x1b[0m, \x1b[31m7 deletions(-)\x1b[0m\r\n",
    "\x1b[1;32m$\x1b[0m cargo build\r\n",
    "\x1b[31merror\x1b[0m: لم يتم العثور على الملف 'main.rs' (السطر ٤٢ من 128)\r\n",
    "قال (نعم) ثم [لا] — والنص \x1b[4mمسطر\x1b[0m و\x1b[1mعريض\x1b[0m\r\n",
    "┌─────────────┬──────────┬────────┐\r\n",
    "│ \x1b[1mالاسم\x1b[0m       │ \x1b[1mالحجم\x1b[0m    │ \x1b[1mنوع\x1b[0m    │\r\n",
    "├─────────────┼──────────┼────────┤\r\n",
    "│ تقرير.pdf   │ 1.2 MB   │ مستند  │\r\n",
    "│ صورة.png    │ 340 KB   │ صورة   │\r\n",
    "└─────────────┴──────────┴────────┘\r\n",
    "\x1b[7m NORMAL \x1b[0m\x1b[44m main.rs                       سطر ٤٢/128 \x1b[0m\r\n",
    "\x1b[1;32m$\x1b[0m ",
);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut scale: Option<f32> = None;
    let mut size = 17.0f32;
    let mut mode = BidiMode::ForceLtr;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--arabic-scale" => scale = args.next().and_then(|v| v.parse().ok()),
            "--font-size" => size = args.next().and_then(|v| v.parse().ok()).unwrap_or(size),
            "--rtl" => mode = BidiMode::ForceRtl,
            "--auto" => mode = BidiMode::Auto,
            _ => {}
        }
    }

    let (rows, cols) = (23usize, 66usize);
    let mut fonts = Fonts::new(size, scale)?;
    let mut term = Terminal::new(rows, cols, 1000);
    term.bidi_mode = mode;
    let mut vt = Vt::new();
    vt.feed(&mut term, SCRIPT.as_bytes());

    let mut renderer = Renderer::new();
    let w = (cols as f32 * fonts.cell_w).ceil() as usize + 2 * renderer.pad_x as usize;
    let h = (rows as f32 * fonts.cell_h).ceil() as usize + 2 * renderer.pad_y as usize;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    renderer.draw(&mut canvas, &term, &mut fonts, &DrawOpts::default());

    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in buf.iter() {
        ppm.push((px >> 16) as u8);
        ppm.push((px >> 8) as u8);
        ppm.push(*px as u8);
    }
    let out = std::env::var("OUT").unwrap_or("/private/tmp/claude-501/render_test.ppm".into());
    std::fs::write(&out, ppm)?;
    println!("wrote {out} {w}x{h}  cell={}x{}", fonts.cell_w, fonts.cell_h);
    Ok(())
}
