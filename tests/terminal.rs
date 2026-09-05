//! Behavioural tests for the VT layer and the grid model.

use arabic_term::color::Color;
use arabic_term::grid::Attrs;
use arabic_term::term::{Modes, Terminal, Vt};

fn run(rows: usize, cols: usize, input: &str) -> Terminal {
    let mut t = Terminal::new(rows, cols, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, input.as_bytes());
    t
}

fn line(t: &Terminal, r: usize) -> String {
    t.grid.screen.rows[r]
        .cells
        .iter()
        .filter(|c| !c.attrs.contains(Attrs::WIDE_SPACER))
        .map(|c| {
            let mut s = String::new();
            s.push(c.ch);
            for m in c.marks() {
                s.push(*m);
            }
            s
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn prints_plain_text() {
    let t = run(4, 20, "hello");
    assert_eq!(line(&t, 0), "hello");
    assert_eq!(t.grid.screen.cursor.col, 5);
}

#[test]
fn arabic_occupies_one_cell_per_letter() {
    // Each Arabic letter is a single grid column; the shaper joins them at
    // render time, not by consuming extra cells.
    let t = run(4, 20, "مرحبا");
    assert_eq!(line(&t, 0), "مرحبا");
    assert_eq!(t.grid.screen.cursor.col, 5);
}

#[test]
fn harakat_attach_to_the_preceding_cell() {
    // بِ is base + kasra: one column, not two.
    let t = run(4, 20, "بِسْمِ");
    assert_eq!(t.grid.screen.cursor.col, 3, "3 base letters => 3 columns");
    assert_eq!(t.grid.screen.rows[0].cells[0].ch, 'ب');
    assert_eq!(t.grid.screen.rows[0].cells[0].marks(), &['\u{0650}']);
    assert_eq!(line(&t, 0), "بِسْمِ");
}

#[test]
fn wide_chars_take_two_columns_with_a_spacer() {
    let t = run(4, 20, "日本");
    assert_eq!(t.grid.screen.cursor.col, 4);
    assert!(t.grid.screen.rows[0].cells[0].attrs.contains(Attrs::WIDE_LEADING));
    assert!(t.grid.screen.rows[0].cells[1].attrs.contains(Attrs::WIDE_SPACER));
}

#[test]
fn autowrap_is_deferred_until_the_next_character() {
    // Writing the last column must not wrap on its own.
    let t = run(4, 5, "abcde");
    assert_eq!(t.grid.screen.cursor.row, 0);
    assert!(t.grid.screen.cursor.pending_wrap);
    let t = run(4, 5, "abcdef");
    assert_eq!(t.grid.screen.cursor.row, 1);
    assert_eq!(line(&t, 1), "f");
    assert!(t.grid.screen.rows[0].wrapped);
}

#[test]
fn sgr_truecolor_semicolon_and_colon_forms() {
    let t = run(2, 40, "\x1b[38;2;10;20;30mA\x1b[38:2::40:50:60mB");
    let c = &t.grid.screen.rows[0].cells;
    assert_eq!(c[0].fg, Color::Rgb(arabic_term::color::Rgb(10, 20, 30)));
    assert_eq!(c[1].fg, Color::Rgb(arabic_term::color::Rgb(40, 50, 60)));
}

#[test]
fn sgr_indexed_and_bright_colors() {
    let t = run(2, 40, "\x1b[31mA\x1b[91mB\x1b[38;5;200mC\x1b[0mD");
    let c = &t.grid.screen.rows[0].cells;
    assert_eq!(c[0].fg, Color::Indexed(1));
    assert_eq!(c[1].fg, Color::Indexed(9));
    assert_eq!(c[2].fg, Color::Indexed(200));
    assert_eq!(c[3].fg, Color::Default);
}

#[test]
fn cursor_addressing_and_erase() {
    let t = run(4, 10, "abcdef\x1b[1;3Hxy");
    assert_eq!(line(&t, 0), "abxyef");
    let t = run(4, 10, "abcdef\x1b[1;3H\x1b[K");
    assert_eq!(line(&t, 0), "ab");
}

#[test]
fn scroll_region_and_reverse_index() {
    let mut t = Terminal::new(5, 10, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[2;4r"); // region rows 2..4
    vt.feed(&mut t, b"\x1b[2;1Ha\r\n\x1b[3;1Hb\r\n\x1b[4;1Hc");
    assert_eq!(line(&t, 1), "a");
    assert_eq!(line(&t, 3), "c");
    // Reverse index at the top margin scrolls the region down.
    vt.feed(&mut t, b"\x1b[2;1H\x1bM");
    assert_eq!(line(&t, 2), "a");
    assert_eq!(line(&t, 0), "", "content outside the region is untouched");
}

#[test]
fn alt_screen_preserves_and_restores_primary() {
    let mut t = Terminal::new(4, 10, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"primary");
    vt.feed(&mut t, b"\x1b[?1049h");
    assert_eq!(line(&t, 0), "", "alt screen starts blank");
    vt.feed(&mut t, b"alt");
    assert_eq!(line(&t, 0), "alt");
    vt.feed(&mut t, b"\x1b[?1049l");
    assert_eq!(line(&t, 0), "primary");
}

#[test]
fn scrollback_captures_lines_leaving_the_top() {
    let mut t = Terminal::new(2, 10, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"one\r\ntwo\r\nthree");
    assert_eq!(t.grid.scrollback.len(), 1);
    assert_eq!(line(&t, 1), "three");
}

#[test]
fn insert_and_delete_characters() {
    let t = run(2, 10, "abcdef\x1b[1;2H\x1b[2@");
    assert_eq!(line(&t, 0), "a  bcdef");
    let t = run(2, 10, "abcdef\x1b[1;2H\x1b[2P");
    assert_eq!(line(&t, 0), "adef");
}

#[test]
fn osc_sets_the_window_title() {
    let t = run(2, 10, "\x1b]0;عنوان\x07");
    assert_eq!(t.title, "عنوان");
}

#[test]
fn device_status_report_replies() {
    let t = run(4, 10, "\x1b[3;5H\x1b[6n");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[3;5R");
}

#[test]
fn modes_toggle() {
    let t = run(2, 10, "\x1b[?25l\x1b[?2004h");
    assert!(!t.modes.contains(Modes::CURSOR_VISIBLE));
    assert!(t.modes.contains(Modes::BRACKETED_PASTE));
}

#[test]
fn erase_inherits_the_current_background() {
    let t = run(2, 10, "\x1b[41m\x1b[2J");
    assert_eq!(t.grid.screen.rows[0].cells[0].bg, Color::Indexed(1));
}

#[test]
fn resize_preserves_content_and_pulls_from_scrollback() {
    let mut t = Terminal::new(2, 10, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"one\r\ntwo\r\nthree");
    assert_eq!(t.grid.scrollback.len(), 1);
    t.resize(3, 10);
    assert_eq!(line(&t, 0), "one", "growing pulls the retired line back");
    assert_eq!(line(&t, 2), "three");
}

#[test]
fn pre_shaped_arabic_is_restored_to_base_letters() {
    // vim/neovim pre-shape Arabic into presentation forms by default. Storing
    // those verbatim leaves the shaper nothing to join, so they are mapped back.
    // U+FEE9 HEH ISOLATED, U+FEDF LAM INITIAL, U+FE8E ALEF FINAL.
    let t = run(2, 20, "\u{FEDF}\u{FE8E}\u{FEE9}");
    assert_eq!(line(&t, 0), "لاه");
    assert_eq!(t.grid.screen.cursor.col, 3, "still one cell per letter");
}

#[test]
fn presentation_form_ligatures_are_left_intact() {
    // U+FEFB LAM WITH ALEF ISOLATED decomposes to two characters. Expanding it
    // would need a cell the writing program never reserved, so it stays as the
    // single glyph it already is.
    let t = run(2, 20, "\u{FEFB}");
    assert_eq!(line(&t, 0), "\u{FEFB}");
    assert_eq!(t.grid.screen.cursor.col, 1);
}

#[test]
fn osc_52_sets_the_clipboard() {
    // "hello" base64 is aGVsbG8=
    let t = run(2, 20, "\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(t.clipboard_write.as_deref(), Some("hello"));
}

#[test]
fn osc_52_handles_utf8_payloads() {
    // "مرحبا" base64
    let t = run(2, 20, "\x1b]52;c;2YXYsdit2KjYpw==\x07");
    assert_eq!(t.clipboard_write.as_deref(), Some("مرحبا"));
}

#[test]
fn osc_52_read_requests_are_refused() {
    // Answering a clipboard read lets anything that can write to the tty
    // exfiltrate what the user last copied.
    let t = run(2, 20, "\x1b]52;c;?\x07");
    assert_eq!(t.clipboard_write, None);
    assert!(t.replies.is_empty(), "must not answer a clipboard query");
}

#[test]
fn osc_52_ignores_malformed_base64() {
    let t = run(2, 20, "\x1b]52;c;!!!not base64!!!\x07");
    assert_eq!(t.clipboard_write, None);
}
