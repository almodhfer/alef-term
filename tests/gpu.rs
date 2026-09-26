//! The GPU compositor must reproduce the software canvas: same layout, same
//! layering, same blends. Render a busy screen both ways and compare pixels.
use arabic_term::color::Palette;
use arabic_term::font::Fonts;
use arabic_term::gpu::Gpu;
use arabic_term::render::{Canvas, DrawOpts, Renderer, Target};
use arabic_term::term::{Terminal, Vt};

const SCRIPT: &str = concat!(
    "\x1b[1;36m~/مشاريع\x1b[0m \x1b[1;32m$\x1b[0m echo \"مرحبا بالعالم\" 🎉 ✅\r\n",
    "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ\r\n",
    "\x1b[32m\u{FE88}نشاء الر\u{FE8C}يسية الم\u{FE86}شرات \u{FE84}فضل \u{FE82}فاق README\x1b[0m\r\n",
    "\x1b[31merror\x1b[0m: لم يتم العثور على الملف 'main.rs' (السطر ٤٢ من 128)\r\n",
    "قال (نعم) ثم [لا] — والنص \x1b[4mمسطر\x1b[0m و\x1b[1mعريض\x1b[0m و\x1b[9mمشطوب\x1b[0m\r\n",
    "┌─────────────┬──────────┐ ▀▄█▌▐░▒▓\r\n",
    "│ \x1b[1mالاسم\x1b[0m       │ \x1b[1mالحجم\x1b[0m    │\r\n",
    "└─────────────┴──────────┘\r\n",
    "\x1b[7m NORMAL \x1b[0m\x1b[44m main.rs   \x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m\r\n",
    // Every underline style, coloured and not.
    "\x1b[4:3;58:2::255:60:60mwavy\x1b[4:2;59m double\x1b[4:4;58;5;33m dotted\x1b[4:5m dashed\x1b[0m خطأ\r\n",
    // A small red sixel picture, so the image path is covered too.
    "\x1bPq#0;2;100;0;0#0~~~~~~~~~~~~-~~~~~~~~~~~~\x1b\\\r\n",
    "\x1b[1;32m$\x1b[0m ",
);

fn draw<T: Target>(target: &mut T, term: &Terminal, fonts: &mut Fonts, renderer: &mut Renderer, opts: &DrawOpts) {
    renderer.draw(target, term, fonts, opts);
}

#[test]
fn gpu_frame_matches_the_software_canvas() {
    for family in [None, Some("Noto Naskh Arabic"), Some("Amiri"), Some("Scheherazade New")] {
        check_gpu_frame(family);
    }
}

fn check_gpu_frame(family: Option<&str>) {
    let (rows, cols) = (12usize, 60usize);
    let mut fonts = Fonts::with_families(17.0, None, None, family).unwrap();
    let mut term = Terminal::new(rows, cols, 1000);
    term.set_cell_px(fonts.cell_w, fonts.cell_h);
    let mut vt = Vt::new();
    vt.feed(&mut term, SCRIPT.as_bytes());

    let mut renderer = Renderer::new();
    renderer.minimum_contrast = 4.5;
    let w = (cols as f32 * fonts.cell_w).ceil() as usize + 2 * renderer.pad_x as usize;
    let h = (rows as f32 * fonts.cell_h).ceil() as usize + 2 * renderer.pad_y as usize;

    let Ok(mut gpu) = Gpu::headless(w as u32, h as u32) else {
        eprintln!("no GPU available; skipping");
        return;
    };
    // Switch with the same layouts and GPU atlas still cached.
    let mut commands = arabic_term::commands::CommandPalette::default();
    commands.push("settings");
    for palette in [Palette::dark(), Palette::light()] {
        term.set_palette(palette);
        for opts in [DrawOpts::default(), DrawOpts { commands: Some(&commands), notice: Some("Settings reloaded"), ..DrawOpts::default() }] {
            let mut cpu = vec![0u32; w * h];
            let mut canvas = Canvas { buf: &mut cpu, width: w, height: h };
            draw(&mut canvas, &term, &mut fonts, &mut renderer, &opts);
            // Two frames: the second hits the glyph atlas and image cache.
            let mut frames = Vec::new();
            for _ in 0..2 {
                gpu.begin();
                draw(&mut gpu, &term, &mut fonts, &mut renderer, &opts);
                frames.push(gpu.read_back().unwrap());
            }

            let bg = term.palette.bg.pack();
            let inked = cpu.iter().filter(|&&p| p != bg).count();
            assert!(inked > w * h / 20, "software frame is nearly blank: {inked} inked pixels");

            for (n, frame) in frames.iter().enumerate() {
                assert_eq!(frame.len(), cpu.len());
                let mut off = Vec::new();
                for (i, (a, b)) in cpu.iter().zip(frame).enumerate() {
                    let d = (0..3)
                        .map(|s| ((a >> (8 * s)) & 0xff).abs_diff((b >> (8 * s)) & 0xff))
                        .max()
                        .unwrap();
                    if d > 2 {
                        let (x, y) = (i % w, i / w);
                        let col = (x as f32 - renderer.pad_x) / fonts.cell_w;
                        let row = (y as f32 - renderer.pad_y) / fonts.cell_h;
                        off.push(format!("({x},{y}) row {row:.2} col {col:.2}: cpu {a:06x} gpu {b:06x}"));
                    }
                }
                // Blends round differently by at most one step; anything more is a
                // misplaced or mislayered primitive.
                assert!(off.is_empty(), "frame {n}: {} pixels differ by more than 2:\n{}", off.len(), off.join("\n"));
            }
        }
    }
}
