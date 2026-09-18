//! Theme changes must reach every primitive, including cached text layouts.
use arabic_term::color::{Palette, Rgb, THEMES};
use arabic_term::font::{Fonts, RasterGlyph};
use arabic_term::render::{DrawOpts, Renderer, Target};
use arabic_term::selection::{Selection, Span};
use arabic_term::sixel::Image;
use arabic_term::term::{Terminal, Vt};

#[derive(Default)]
struct Paint {
    background: Option<Rgb>,
    rects: Vec<Rgb>,
    glyphs: Vec<Rgb>,
}

impl Target for Paint {
    fn width(&self) -> usize {
        800
    }
    fn height(&self) -> usize {
        200
    }
    fn clear(&mut self, c: Rgb) {
        self.background = Some(c);
    }
    fn fill_rect(&mut self, _: f32, _: f32, _: f32, _: f32, c: Rgb) {
        self.rects.push(c);
    }
    fn glyph(&mut self, g: &RasterGlyph, _: i32, _: i32, fg: Rgb) {
        if !g.color {
            self.glyphs.push(fg);
        }
    }
    fn image(&mut self, _: &Image, _: i32, _: i32) {}
}

struct Rig {
    fonts: Fonts,
    renderer: Renderer,
    term: Terminal,
}

impl Rig {
    fn new(text: &str) -> Self {
        let mut term = Terminal::new(1, 12, 10);
        Vt::new().feed(&mut term, text.as_bytes());
        Self {
            fonts: Fonts::new(18.0, None).unwrap(),
            renderer: Renderer::new(),
            term,
        }
    }

    fn draw(&mut self, opts: DrawOpts) -> Paint {
        let mut paint = Paint::default();
        self.renderer
            .draw(&mut paint, &self.term, &mut self.fonts, &opts);
        paint
    }
}

fn opts() -> DrawOpts<'static> {
    DrawOpts {
        cursor_on: false,
        ..Default::default()
    }
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let luminance = |c: Rgb| {
        let linear = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn dim_text_and_decorations_follow_the_background_in_every_theme() {
    let mut rig = Rig::new("\x1b[2;4;9mA─");
    for theme in THEMES {
        let p = Palette::from_theme(theme);
        rig.term.set_palette(p.clone());
        let paint = rig.draw(opts());
        let fg = paint.glyphs[0];
        assert!(
            contrast(fg, p.bg) < contrast(p.fg, p.bg),
            "{}: dim text should be fainter",
            theme.name
        );
        assert!(paint.glyphs.iter().all(|&c| c == fg));
        assert!(!paint.rects.is_empty());
        assert!(
            paint.rects.iter().all(|&c| c == fg),
            "{}: lines and text should share their dim colour",
            theme.name
        );
    }

    let mut rig = Rig::new("\x1b[38;2;20;40;60;48;2;200;220;240;2;4mA─");
    let paint = rig.draw(opts());
    assert!(
        paint.glyphs.iter().all(|&c| c == Rgb(110, 130, 150)),
        "dim toward the cell background, including explicit RGB"
    );
}

#[test]
fn reverse_video_is_applied_once_and_decorations_follow_it() {
    let mut rig = Rig::new("");
    for theme in THEMES {
        let p = Palette::from_theme(theme);
        for (mode, foreground, background) in [
            ("\x1b[7m", p.bg, p.fg),
            ("\x1b[?5h", p.bg, p.fg),
            ("\x1b[?5h\x1b[7m", p.fg, p.bg),
        ] {
            rig.term = Terminal::new(1, 12, 10);
            Vt::new().feed(&mut rig.term, format!("{mode}\x1b[4;9mA─").as_bytes());
            rig.term.set_palette(p.clone());
            let paint = rig.draw(opts());
            assert!(
                paint.glyphs.iter().all(|&c| c == foreground),
                "{}: {mode:?}",
                theme.name
            );
            assert!(
                paint.rects.contains(&foreground),
                "decorations use the displayed foreground"
            );
            assert!(paint.rects.contains(&background) || paint.background == Some(background));
        }
    }
}

#[test]
fn highlights_keep_text_borders_and_decorations_readable_after_theme_switches() {
    let mut rig = Rig::new("\x1b[7;4;9mA─");
    let spans = [Span {
        line: 0,
        start: 0,
        end: 2,
    }];
    for theme in THEMES {
        let p = Palette::from_theme(theme);
        rig.term.set_palette(p.clone());
        for (draw_opts, bg) in [
            (
                DrawOpts {
                    highlights: &spans,
                    ..opts()
                },
                p.search_bg,
            ),
            (
                DrawOpts {
                    highlights: &spans,
                    current: &spans,
                    ..opts()
                },
                p.search_current_bg,
            ),
            (
                DrawOpts {
                    selection: Some(Selection {
                        start: (0, 0),
                        end: (0, 1),
                        block: false,
                    }),
                    highlights: &spans,
                    current: &spans,
                    ..opts()
                },
                p.selection_bg,
            ),
        ] {
            let paint = rig.draw(draw_opts);
            let fg = paint.glyphs[0];
            assert!(
                contrast(fg, bg) >= 4.5,
                "{}: highlight text disappears",
                theme.name
            );
            assert!(paint.glyphs.iter().all(|&c| c == fg));
            assert!(paint.rects.contains(&bg));
            assert!(paint.rects.contains(&fg));
            assert!(
                paint.rects.iter().all(|&c| c == bg || c == fg),
                "{}: all highlight primitives agree",
                theme.name
            );
        }
    }
}

#[test]
fn selection_preserves_readable_colours_and_honours_an_explicit_foreground() {
    let selection = Some(Selection {
        start: (0, 0),
        end: (0, 1),
        block: false,
    });
    let mut rig = Rig::new("\x1b[38;2;120;0;0;4;9mA─");
    rig.term.set_palette(Palette::light());
    let paint = rig.draw(DrawOpts {
        selection,
        ..opts()
    });
    assert!(
        paint.glyphs.iter().all(|&c| c == Rgb(120, 0, 0)),
        "readable syntax colour is retained"
    );

    rig.term.palette.selection_fg = Some(Rgb(30, 60, 90));
    let paint = rig.draw(DrawOpts {
        selection,
        ..opts()
    });
    assert!(paint.glyphs.iter().all(|&c| c == Rgb(30, 60, 90)));
    assert!(paint
        .rects
        .iter()
        .all(|&c| c == rig.term.palette.selection_bg || c == Rgb(30, 60, 90)));
}

#[test]
fn a_custom_block_cursor_keeps_its_character_readable() {
    let mut rig = Rig::new("A\r");
    for p in [Palette::dark(), Palette::light()] {
        rig.term.set_palette(p);
        rig.term.palette.cursor = rig.term.palette.bg;
        let paint = rig.draw(DrawOpts::default());
        let fg = *paint.glyphs.last().unwrap();
        assert!(contrast(fg, rig.term.palette.cursor) >= 4.5);
    }
}

#[test]
fn theme_switches_recolour_indexed_text_and_leave_literal_colours_intact() {
    let mut rig = Rig::new("\x1b[31mA\x1b[38;5;196mB\x1b[38;2;12;34;56;48;2;23;45;67mC");
    for theme in THEMES {
        rig.term.set_palette(Palette::from_theme(theme));
        let paint = rig.draw(opts());
        assert_eq!(
            paint.glyphs,
            [theme.ansi[1], Rgb(255, 0, 0), Rgb(12, 34, 56)]
        );
        assert!(paint.rects.contains(&Rgb(23, 45, 67)));
    }
}

#[test]
fn pastel_application_text_stays_readable_on_light_backgrounds() {
    // A pale code/link colour retained by an application started in dark mode.
    let mut rig = Rig::new("\x1b[38;2;180;190;254;4mA─سلام");
    let original = Rgb(180, 190, 254);
    rig.term.set_palette(Palette::light());
    rig.renderer.minimum_contrast = 4.5;
    let paint = rig.draw(opts());
    assert!(!paint.glyphs.is_empty());
    let ink = paint.glyphs[0];
    assert!(contrast(ink, rig.term.palette.bg) >= 4.5);
    assert!(ink.2 > ink.1 && ink.1 > ink.0, "keep the original blue hue");
    assert!(paint.glyphs.iter().all(|&c| c == ink));
    assert!(paint.rects.iter().all(|&c| c == ink));
    assert_eq!(rig.term.grid.viewport_row(0).cells[0].fg, arabic_term::color::Color::Rgb(original));
    // Cached layouts/colours must follow a theme switch and an opt-out.
    rig.term.set_palette(Palette::dark());
    assert!(rig.draw(opts()).glyphs.iter().all(|&c| c == original));
    rig.term.set_palette(Palette::light());
    rig.renderer.minimum_contrast = 1.0;
    assert!(rig.draw(opts()).glyphs.iter().all(|&c| c == original));
}

#[test]
fn contrast_respects_cell_backgrounds_dim_and_concealment() {
    let mut rig = Rig::new("\x1b[38;2;180;190;254;48;2;240;240;240mA");
    rig.renderer.minimum_contrast = 4.5;
    let bg = Rgb(240, 240, 240);
    let paint = rig.draw(opts());
    assert!(paint.rects.contains(&bg), "backgrounds must never be recoloured");
    assert!(paint.glyphs.iter().all(|&c| contrast(c, bg) >= 4.5));
    rig.term = Terminal::new(1, 12, 10);
    Vt::new().feed(&mut rig.term, b"\x1b[2;38;2;180;190;254;48;2;240;240;240mA");
    let dim = rig.draw(opts()).glyphs[0];
    assert!(contrast(dim, bg) >= 3.0 && contrast(dim, bg) < 4.5);
    rig.term = Terminal::new(1, 12, 10);
    Vt::new().feed(&mut rig.term, b"\x1b[38;2;240;240;240;48;2;240;240;240mA\x1b[8mB");
    assert_eq!(rig.draw(opts()).glyphs, [bg], "identical colours and SGR conceal stay concealed");
}

#[test]
fn contrast_preserves_block_art_and_powerline_surfaces() {
    let mut rig = Rig::new("\x1b[38;2;180;190;254m█░\u{e0b0}");
    rig.term.set_palette(Palette::light());
    rig.renderer.minimum_contrast = 4.5;
    let paint = rig.draw(opts());
    assert!(paint.rects.iter().all(|&c| c == Rgb(180, 190, 254)));
    assert!(paint.glyphs.iter().all(|&c| c == Rgb(180, 190, 254)));
}

#[test]
fn contrast_adjustment_chooses_a_reachable_value_and_preserves_readable_ink() {
    for bg in [Rgb(250, 250, 247), Rgb(20, 22, 27), Rgb(128, 128, 128)] {
        for fg in [Rgb(180, 190, 254), Rgb(250, 220, 160), Rgb(30, 40, 50)] {
            let ink = fg.ensure_contrast(bg, 4.5);
            assert!(contrast(ink, bg) >= 4.5);
            if contrast(fg, bg) >= 4.5 { assert_eq!(ink, fg); }
            assert_eq!(fg.ensure_contrast(bg, 1.0), fg);
        }
    }
    let bg = Rgb(128, 128, 128);
    let best = Rgb(130, 140, 150).ensure_contrast(bg, 21.0);
    assert_eq!(best, Rgb(0, 0, 0), "unreachable ratios use the stronger endpoint");
}
