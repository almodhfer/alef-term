//! Render a showcase screen offline to a PPM, without opening a window.
//! Useful for eyeballing layout changes and for regression screenshots.
use alef_term::font::Fonts;
use alef_term::color::{default_theme, find_theme, Palette};
use alef_term::render::{Canvas, DrawOpts, Renderer};
use alef_term::selection::{Selection, Span};
use alef_term::term::{BidiMode, Terminal, Vt};

const SCRIPT: &str = concat!(
    "\x1b[1;36m~/مشاريع\x1b[0m \x1b[1;32m$\x1b[0m echo \"مرحبا بالعالم\"\r\n",
    "مرحبا بالعالم\r\n",
    "افتح الملف README ثم احفظ التغييرات\r\n",
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
    "\x1b[2;4mDim text · نص خافت ───\x1b[0m\r\n",
    "\x1b[1;32m$\x1b[0m ",
);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut scale: Option<f32> = None;
    let mut size = 17.0f32;
    let mut mode = BidiMode::default();
    let mut dpi = 1.0f32;
    let mut theme = default_theme();
    let mut highlights = false;
    let mut commands = false;
    let mut contrast_demo = false;
    let mut minimum_contrast = 4.5;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--arabic-scale" => scale = args.next().and_then(|v| v.parse().ok()),
            "--font-size" => size = args.next().and_then(|v| v.parse().ok()).unwrap_or(size),
            "--rtl" => mode = BidiMode::ForceRtl,
            "--auto" => mode = BidiMode::Auto,
            "--ltr" => mode = BidiMode::ForceLtr,
            "--dpi" => dpi = args.next().and_then(|v| v.parse().ok()).unwrap_or(1.0),
            "--theme" => {
                let name = args.next().unwrap_or_default();
                theme = find_theme(&name).ok_or_else(|| anyhow::anyhow!("unknown theme: {name}"))?;
            }
            "--highlights" => highlights = true,
            "--commands" => commands = true,
            "--contrast-demo" => contrast_demo = true,
            "--minimum-contrast" => minimum_contrast = args.next().and_then(|v| v.parse().ok()).unwrap_or(4.5),
            _ => {}
        }
    }

    let (rows, cols) = if contrast_demo { (13usize, 72usize) } else { (24usize, 66usize) };
    let mut fonts = Fonts::new(size * dpi, scale)?;
    let mut term = Terminal::new(rows, cols, 1000);
    term.set_palette(Palette::from_theme(theme));
    term.bidi_mode = mode;
    let mut vt = Vt::new();
    let demo = concat!(
        "\x1b[1mLight theme · application colour compatibility\x1b[0m\r\n",
        "\r\n",
        "File: \x1b[38;2;180;190;254mmain.rs   README.md   src/render.rs\x1b[0m\r\n",
        "\x1b[38;2;180;190;254mافتح الملف README ثم احفظ التغييرات\x1b[0m\r\n",
        "\x1b[38;2;249;226;175mWorking · 1 shell · 2 checks remaining\x1b[0m\r\n",
        "\x1b[38;2;150;150;160mSecondary status and timestamps\x1b[0m\r\n",
        "\x1b[2mDim text stays secondary · نص خافت\x1b[0m\r\n",
        "\r\n",
        "\x1b[38;2;180;190;254;48;2;45;50;65m Explicit dark panel keeps its own background \x1b[0m\r\n",
        "\x1b[38;2;180;190;254;4mUnderlined links · روابط واضحة\x1b[0m\r\n",
        "\r\n",
        "\x1b[38;2;40;80;140mAlready-readable colours stay unchanged.\x1b[0m\r\n",
    );
    vt.feed(&mut term, if contrast_demo { demo.as_bytes() } else { SCRIPT.as_bytes() });

    let mut renderer = Renderer::new();
    renderer.minimum_contrast = minimum_contrast;
    renderer.pad_x *= dpi;
    renderer.pad_y *= dpi;
    let w = (cols as f32 * fonts.cell_w).ceil() as usize + 2 * renderer.pad_x as usize;
    let h = (rows as f32 * fonts.cell_h).ceil() as usize + 2 * renderer.pad_y as usize;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    let spans = [Span { line: 1, start: 0, end: 13 }, Span { line: 12, start: 0, end: 5 }];
    let mut opts = if highlights {
        DrawOpts {
            selection: Some(Selection { start: (20, 0), end: (20, 7), block: false }),
            highlights: &spans,
            current: &spans[1..],
            ..Default::default()
        }
    } else {
        DrawOpts::default()
    };
    let command_palette = alef_term::commands::CommandPalette::default();
    if commands { opts.commands = Some(&command_palette); }
    renderer.draw(&mut canvas, &term, &mut fonts, &opts);

    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in buf.iter() {
        ppm.push((px >> 16) as u8);
        ppm.push((px >> 8) as u8);
        ppm.push(*px as u8);
    }
    let out = std::env::var("OUT").unwrap_or_else(|_| std::env::temp_dir()
        .join("alef-term-render.ppm").to_string_lossy().into_owned());
    std::fs::write(&out, ppm)?;
    println!("wrote {out} {w}x{h}  cell={}x{}", fonts.cell_w, fonts.cell_h);
    Ok(())
}
