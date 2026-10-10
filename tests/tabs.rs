//! The tab bar: where it sits, and how it marks the tab on show.
use alef_term::font::Fonts;
use alef_term::render::{tab_bar_height, Canvas, DrawOpts, Renderer, TabBar};
use alef_term::term::{Terminal, Vt};

const W: usize = 480;
const H: usize = 200;

fn render(tabs: Option<TabBar>) -> (Vec<u32>, Terminal, Fonts, Renderer) {
    let mut fonts = Fonts::new(18.0, None).unwrap();
    let mut term = Terminal::new(2, 30, 10);
    Vt::new().feed(&mut term, b"hello");
    let mut renderer = Renderer::new();
    let mut buf = vec![0u32; W * H];
    let opts = DrawOpts { tabs, ..DrawOpts::default() };
    renderer.draw(&mut Canvas { buf: &mut buf, width: W, height: H }, &term, &mut fonts, &opts);
    (buf, term, fonts, renderer)
}

/// The first pixel row at or below `from` with text ink in the first cells.
fn first_ink(buf: &[u32], bg: u32, x0: usize, x1: usize, from: usize) -> usize {
    (from..H).find(|&y| (x0..x1).any(|x| buf[y * W + x] != bg)).expect("some ink")
}

#[test]
fn the_grid_moves_down_by_exactly_the_bar() {
    let titles = vec!["Shell 1".to_string(), "مشاريع".to_string()];
    let (plain, term, fonts, renderer) = render(None);
    let (tabbed, _, _, _) = render(Some(TabBar { titles: &titles, active: 1 }));
    let bg = term.palette.bg.pack();
    let bar = tab_bar_height(&fonts) as usize;
    let (x0, x1) = (renderer.pad_x as usize, renderer.pad_x as usize + 5 * fonts.cell_w as usize);
    assert_eq!(first_ink(&tabbed, bg, x0, x1, bar), first_ink(&plain, bg, x0, x1, 0) + bar);
}

#[test]
fn the_active_tab_is_marked_and_the_others_are_not() {
    let titles = vec!["Shell 1".to_string(), "مشاريع".to_string()];
    let (buf, term, _, _) = render(Some(TabBar { titles: &titles, active: 1 }));
    let accent = term.palette.cursor.pack();
    // Two tabs share the width; the accent runs along the top of the second.
    assert_eq!(buf[W + 3 * W / 4], accent);
    assert_ne!(buf[W + W / 4], accent);
}
