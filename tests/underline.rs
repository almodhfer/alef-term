//! Underline styles and colours: what `SGR 4:n`, 21, 24, 58 and 59 store, and
//! how a wavy underline is drawn.
use arabic_term::color::{Color, Rgb};
use arabic_term::font::Fonts;
use arabic_term::grid::Attrs;
use arabic_term::render::{Canvas, DrawOpts, Renderer};
use arabic_term::term::{Terminal, Vt};

const STYLES: Attrs = Attrs::UNDERLINE_STYLES;

fn run(input: &str) -> Terminal {
    let mut t = Terminal::new(2, 20, 10);
    Vt::new().feed(&mut t, input.as_bytes());
    t
}

#[test]
fn underline_styles_and_colours_are_kept_per_cell() {
    let t = run("\x1b[4:3;58:2::255:0:0ma\x1b[4:0mb\x1b[21mc\x1b[24;4md\x1b[59me\x1b[0mf\x1b[4:5;58;5;33mg\x1b[4mh");
    let cell = |i: usize| &t.grid.screen.rows[0].cells[i];
    let red = Some(Color::Rgb(Rgb(255, 0, 0)));

    assert_eq!(cell(0).attrs & (Attrs::UNDERLINE | STYLES), Attrs::UNDERLINE | Attrs::UNDERLINE_CURLY);
    assert_eq!(cell(0).underline_color(), red);
    assert!(!cell(1).attrs.contains(Attrs::UNDERLINE), "4:0 turns the underline off");
    assert_eq!(cell(2).attrs & (Attrs::UNDERLINE | STYLES), Attrs::UNDERLINE | Attrs::UNDERLINE_DOUBLE);
    assert_eq!(cell(3).attrs & (Attrs::UNDERLINE | STYLES), Attrs::UNDERLINE, "24 then 4 is a plain line");
    assert_eq!(cell(3).underline_color(), red, "the colour outlives the style");
    assert_eq!(cell(4).underline_color(), None, "59 returns to the text colour");
    assert!(!cell(5).attrs.intersects(Attrs::UNDERLINE | STYLES), "0 resets everything");
    assert_eq!(cell(6).attrs & STYLES, Attrs::UNDERLINE_DASHED);
    assert_eq!(cell(6).underline_color(), Some(Color::Indexed(33)));
    assert_eq!(cell(7).attrs & STYLES, Attrs::empty(), "plain 4 replaces the dashed style");
}

#[test]
fn a_wide_character_carries_its_underline_colour_on_both_halves() {
    let t = run("\x1b[4:3;58;5;196m中");
    let row = &t.grid.screen.rows[0];
    assert_eq!(row.cells[0].underline_color(), Some(Color::Indexed(196)));
    assert_eq!(row.cells[1].underline_color(), Some(Color::Indexed(196)));
}

#[test]
fn a_wavy_underline_runs_unbroken_under_its_cells_in_its_own_colour() {
    let (w, h) = (400usize, 120usize);
    let mut fonts = Fonts::new(18.0, None).unwrap();
    let mut term = Terminal::new(1, 20, 10);
    Vt::new().feed(&mut term, b"\x1b[4:3;58:2::255:0:0mwave\x1b[0m");
    let mut renderer = Renderer::new();
    let mut buf = vec![0u32; w * h];
    renderer.draw(&mut Canvas { buf: &mut buf, width: w, height: h }, &term, &mut fonts, &DrawOpts::default());

    let red = |p: u32| (p >> 16) & 255 > 120 && (p >> 8) & 255 < 90 && p & 255 < 90;
    let x0 = renderer.pad_x as usize;
    let x1 = x0 + 4 * fonts.cell_w as usize;
    for x in x0..x1 {
        assert!((0..h).any(|y| red(buf[y * w + x])), "a gap in the wave at x={x}");
    }
    let text_bottom = renderer.pad_y as usize + fonts.baseline as usize - 3;
    assert!(!(x0..x1).any(|x| (0..text_bottom).any(|y| red(buf[y * w + x]))), "the wave runs into the text");
    assert!(!(x1 + 2..w).any(|x| (0..h).any(|y| red(buf[y * w + x]))), "the wave runs past its cells");
}
