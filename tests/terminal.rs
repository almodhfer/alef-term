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

#[test]
fn kitty_keyboard_flags_push_and_pop() {
    // Nesting matters: an editor inside a multiplexer pushes its flags and pops
    // them on exit, and the outer program's mode must come back.
    let mut t = Terminal::new(3, 20, 10);
    let mut vt = Vt::new();
    assert_eq!(t.kitty_flags(), 0);
    vt.feed(&mut t, b"\x1b[>1u");
    assert_eq!(t.kitty_flags(), 1);
    vt.feed(&mut t, b"\x1b[>5u");
    assert_eq!(t.kitty_flags(), 5);
    vt.feed(&mut t, b"\x1b[<u");
    assert_eq!(t.kitty_flags(), 1, "popping restores the outer mode");
    vt.feed(&mut t, b"\x1b[<u");
    assert_eq!(t.kitty_flags(), 0);
}

#[test]
fn kitty_keyboard_query_is_answered() {
    let mut t = Terminal::new(3, 20, 10);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[>3u\x1b[?u");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?3u");
}

#[test]
fn kitty_keyboard_set_modes_or_and_clear() {
    let mut t = Terminal::new(3, 20, 10);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[=1;1u"); // replace with 1
    assert_eq!(t.kitty_flags(), 1);
    vt.feed(&mut t, b"\x1b[=4;2u"); // set bit 4
    assert_eq!(t.kitty_flags(), 5);
    vt.feed(&mut t, b"\x1b[=1;3u"); // clear bit 1
    assert_eq!(t.kitty_flags(), 4);
}

#[test]
fn plain_csi_u_still_restores_the_cursor() {
    // The kitty sequences are all introduced by an intermediate; bare CSI u
    // must keep its original meaning.
    let t = run(3, 20, "\x1b[2;5H\x1b[s\x1b[1;1H\x1b[u");
    assert_eq!(t.grid.screen.cursor.row, 1);
    assert_eq!(t.grid.screen.cursor.col, 4);
}

#[test]
fn resizing_while_on_the_alt_screen_keeps_both_buffers_sane() {
    // A multiplexer or editor lives on the alt screen; resizing under it must
    // not corrupt either buffer or leave the cursor out of bounds.
    let mut t = Terminal::new(10, 40, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"primary content\r\nsecond line");
    vt.feed(&mut t, b"\x1b[?1049h");
    vt.feed(&mut t, b"\x1b[9;30Halt screen bottom");

    t.resize(5, 20);
    assert_eq!(t.grid.rows, 5);
    assert_eq!(t.grid.cols, 20);
    assert!(t.grid.screen.cursor.row < 5, "cursor row clamped into the new size");
    assert!(t.grid.screen.cursor.col < 20, "cursor col clamped into the new size");
    for r in &t.grid.screen.rows {
        assert_eq!(r.cells.len(), 20, "every row resized");
    }

    t.resize(12, 60);
    vt.feed(&mut t, b"\x1b[?1049l");
    assert_eq!(t.grid.rows, 12);
    for r in &t.grid.screen.rows {
        assert_eq!(r.cells.len(), 60);
    }
    assert_eq!(line(&t, 0), "primary content", "primary buffer survived");
}

#[test]
fn scroll_region_is_clamped_when_the_screen_shrinks() {
    let mut t = Terminal::new(20, 40, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[5;18r");
    t.resize(8, 40);
    assert!(t.grid.screen.scroll_bot <= 8);
    assert!(t.grid.screen.scroll_top < t.grid.screen.scroll_bot);
    // And the terminal still works afterwards.
    vt.feed(&mut t, b"\x1b[1;1Hafter resize");
    assert_eq!(line(&t, 0), "after resize");
}

#[test]
fn a_sixel_sequence_places_an_image_and_moves_the_cursor_below_it() {
    let mut t = Terminal::new(20, 40, 100);
    t.set_cell_px(8.0, 16.0);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1bPq\"1;1;16;32#1;2;100;0;0!16~-!16~-!16~-!16~-!16~-!16~\x1b\\");

    assert_eq!(t.images.len(), 1);
    let img = &t.images[0];
    assert_eq!(img.image.width, 16);
    assert!(img.image.height >= 32);
    assert_eq!(img.line, 0, "placed on the line the cursor was on");
    // 36 pixels tall over 16px cells is 3 rows, so the cursor lands below.
    assert!(t.grid.screen.cursor.row >= 2, "cursor moved past the image");
    assert_eq!(t.grid.screen.cursor.col, 0);
}

#[test]
fn an_image_stays_pinned_to_its_text_when_the_screen_scrolls() {
    let mut t = Terminal::new(4, 20, 100);
    t.set_cell_px(8.0, 16.0);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1bPq#1;2;0;100;0!4~\x1b\\");
    let placed_at = t.images[0].line;
    assert_eq!(t.grid.scrolled, 0);

    // Push the image off the top.
    for _ in 0..8 {
        vt.feed(&mut t, b"x\r\n");
    }
    assert!(t.grid.scrolled > 0, "screen scrolled");
    // The image keeps its absolute line, so it scrolls with the text rather
    // than staying stuck to a screen row.
    assert_eq!(t.images[0].line, placed_at);
}

#[test]
fn a_non_sixel_dcs_is_ignored() {
    // DECRQSS and friends must not be mistaken for image data.
    let mut t = Terminal::new(5, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1bP$qm\x1b\\hello");
    assert!(t.images.is_empty());
    assert_eq!(line(&t, 0), "hello");
}

#[test]
fn images_are_pruned_once_they_fall_out_of_scrollback() {
    let mut t = Terminal::new(4, 20, 8);
    t.set_cell_px(8.0, 16.0);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1bPq#1;2;0;0;100!4~\x1b\\");
    assert_eq!(t.images.len(), 1);
    for _ in 0..300 {
        vt.feed(&mut t, b"line\r\n");
    }
    t.prune_images();
    assert!(t.images.is_empty(), "an image far outside the scrollback is dropped");
}

#[test]
fn decscusr_sets_the_cursor_shape_and_blink() {
    use arabic_term::term::{CursorShape, CursorStyle};
    let t = run(2, 10, "\x1b[5 q");
    assert_eq!(t.cursor_style, CursorStyle { shape: CursorShape::Bar, blink: Some(true) });
    let t = run(2, 10, "\x1b[4 q");
    assert_eq!(t.cursor_style, CursorStyle { shape: CursorShape::Underline, blink: Some(false) });
    // Zero hands the choice back to the terminal's configuration.
    let t = run(2, 10, "\x1b[3 q\x1b[0 q");
    assert_eq!(t.cursor_style, CursorStyle::default());
    // An unknown value changes nothing.
    let t = run(2, 10, "\x1b[6 q\x1b[9 q");
    assert_eq!(t.cursor_style.shape, CursorShape::Bar);
}

#[test]
fn osc_colour_queries_are_answered_in_xterm_form() {
    let mut t = run(2, 10, "");
    let bg = t.palette.bg;
    t.replies.clear();
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b]11;?\x1b\\");
    let want = format!("\x1b]11;rgb:{:04x}/{:04x}/{:04x}\x1b\\", bg.0 as u32 * 257, bg.1 as u32 * 257, bg.2 as u32 * 257);
    assert_eq!(String::from_utf8_lossy(&t.replies), want);
    // A BEL-terminated query gets a BEL-terminated answer.
    t.replies.clear();
    vt.feed(&mut t, b"\x1b]10;?\x07");
    assert!(t.replies.starts_with(b"\x1b]10;rgb:") && t.replies.ends_with(b"\x07"));
    t.replies.clear();
    vt.feed(&mut t, b"\x1b]4;1;?\x07");
    assert!(t.replies.starts_with(b"\x1b]4;1;rgb:"));
}

#[test]
fn osc_colour_sets_and_resets() {
    use arabic_term::color::Rgb;
    let mut t = run(2, 10, "");
    let (fg0, bg0) = (t.palette.fg, t.palette.bg);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b]10;rgb:ff/00/00\x07\x1b]11;#00ff00\x07\x1b]12;rgb:0000/0000/ffff\x07");
    assert_eq!(t.palette.fg, Rgb(255, 0, 0));
    assert_eq!(t.palette.bg, bg0, "unsupported #rrggbb form is ignored, not misparsed");
    assert_eq!(t.palette.cursor, Rgb(0, 0, 255));
    vt.feed(&mut t, b"\x1b]4;1;rgb:12/34/56\x07");
    assert_eq!(t.palette.colors[1], Rgb(0x12, 0x34, 0x56));
    vt.feed(&mut t, b"\x1b]110\x07\x1b]104\x07");
    assert_eq!(t.palette.fg, fg0);
    assert_ne!(t.palette.colors[1], Rgb(0x12, 0x34, 0x56));
}

#[test]
fn bel_is_counted_for_the_ui() {
    let t = run(2, 10, "a\x07b\x07\x07");
    assert_eq!(t.bells, 3);
    assert_eq!(line(&t, 0), "ab");
}

/// Every row of the screen, including trailing blanks trimmed.
fn screen(t: &Terminal) -> Vec<String> {
    (0..t.grid.rows).map(|r| line(t, r)).collect()
}

#[test]
fn narrowing_rewraps_long_lines_instead_of_cutting_them() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"0123456789abcdefghij_and_more");
    assert_eq!(screen(&t)[..2], ["0123456789abcdefghij", "_and_more"]);
    assert!(t.grid.screen.rows[0].wrapped);

    t.resize(4, 10);
    assert_eq!(screen(&t), ["0123456789", "abcdefghij", "_and_more", ""]);
    assert_eq!((t.grid.screen.cursor.row, t.grid.screen.cursor.col), (2, 9));

    // Widening joins the pieces back into one line.
    t.resize(4, 40);
    assert_eq!(screen(&t)[0], "0123456789abcdefghij_and_more");
    assert_eq!((t.grid.screen.cursor.row, t.grid.screen.cursor.col), (0, 29));
    assert!(!t.grid.screen.rows[0].wrapped);
}

#[test]
fn explicit_newlines_survive_reflow() {
    let mut t = Terminal::new(6, 30, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"short\r\nthis line is longer than ten\r\nend");
    t.resize(6, 10);
    assert_eq!(screen(&t), ["short", "this line", "is longer", "than ten", "end", ""]);
    t.resize(6, 30);
    assert_eq!(screen(&t)[..3], ["short", "this line is longer than ten", "end"]);
}

#[test]
fn reflow_moves_text_between_screen_and_scrollback() {
    let mut t = Terminal::new(3, 10, 100);
    let mut vt = Vt::new();
    // Three logical lines, the middle one wrapping onto two rows: four rows
    // for a three-row screen, so one goes to scrollback.
    vt.feed(&mut t, b"one\r\n0123456789abcdef\r\nthree");
    assert_eq!(t.grid.scrollback.len(), 1);
    assert_eq!(screen(&t), ["0123456789", "abcdef", "three"]);

    // Wide enough for the long line to fit on one row: three rows in total,
    // so the retired line comes back.
    t.resize(3, 20);
    assert_eq!(t.grid.scrollback.len(), 0);
    assert_eq!(screen(&t), ["one", "0123456789abcdef", "three"]);
    assert_eq!((t.grid.screen.cursor.row, t.grid.screen.cursor.col), (2, 5));
    // And the terminal keeps working at the right place.
    vt.feed(&mut t, b"!");
    assert_eq!(line(&t, 2), "three!");

    // Narrow again: the first line retires once more.
    t.resize(3, 10);
    assert_eq!(t.grid.scrollback.len(), 1);
    assert_eq!(screen(&t), ["0123456789", "abcdef", "three!"]);
}

#[test]
fn a_short_session_stays_at_the_top_when_reflowed() {
    let mut t = Terminal::new(10, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"$ ls\r\nfile\r\n$ ");
    t.resize(10, 15);
    assert_eq!(screen(&t)[..3], ["$ ls", "file", "$"]);
    assert_eq!((t.grid.screen.cursor.row, t.grid.screen.cursor.col), (2, 2));
    assert_eq!(t.grid.scrollback.len(), 0);
}

#[test]
fn wide_characters_are_not_split_by_reflow() {
    let mut t = Terminal::new(3, 10, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, "abc日本語xyz".as_bytes());
    // 3 + 6 + 3 = 12 cells: at 6 columns the second wide char would straddle
    // the margin, so it must move down whole, leaving a blank.
    t.resize(3, 6);
    assert_eq!(line(&t, 0), "abc日");
    assert_eq!(line(&t, 1), "本語xy");
    assert_eq!(line(&t, 2), "z");
    assert!(t.grid.screen.rows[0].wrapped);
    assert!(t.grid.screen.rows[0].cells[5].is_blank(), "filler where the wide char did not fit");
    assert!(t.grid.screen.rows[1].cells[0].attrs.contains(Attrs::WIDE_LEADING));
    t.resize(3, 20);
    assert_eq!(line(&t, 0), "abc日本語xyz");
}

#[test]
fn reflow_leaves_the_alternate_screen_alone_but_fixes_the_primary_beneath() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"0123456789abcdefghijKLM");
    vt.feed(&mut t, b"\x1b[?1049h\x1b[1;1Hfull screen program");
    t.resize(4, 10);
    assert_eq!(line(&t, 0), "full scree", "alternate screen is clamped, not rewrapped");
    vt.feed(&mut t, b"\x1b[?1049l");
    assert_eq!(screen(&t)[..3], ["0123456789", "abcdefghij", "KLM"]);
}

#[test]
fn images_follow_their_line_through_a_reflow() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    t.set_cell_px(8.0, 16.0);
    vt.feed(&mut t, b"0123456789abcdefghij_wrapped\r\n");
    vt.feed(&mut t, b"\x1bPq#0;2;100;0;0#0~~~~~~-\x1b\\");
    assert_eq!(t.images.len(), 1);
    let before = t.images[0].line;
    assert_eq!(before, 2, "drawn on the third row");
    t.resize(6, 10);
    // The long line now takes three rows, so the image sits on the fourth.
    assert_eq!(t.images[0].line, before + 1);
}

#[test]
fn reflow_is_fast_enough_for_live_resizing() {
    let mut t = Terminal::new(50, 200, 10_000);
    let mut vt = Vt::new();
    for i in 0..5000 {
        vt.feed(&mut t, format!("line {i} {}\r\n", "x".repeat(150 + i % 100)).as_bytes());
    }
    let start = std::time::Instant::now();
    t.resize(50, 120);
    t.resize(50, 200);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(ms < 400.0, "two reflows of 5k long lines took {ms:.0} ms");
    eprintln!("two reflows of 5k long lines: {ms:.1} ms");
    assert!(screen(&t).iter().any(|r| r.starts_with("line 4999 ")), "the last line is on screen");
    assert_eq!(t.grid.screen.cursor.col, 0);
}

#[test]
fn osc7_records_the_working_directory() {
    let t = run(2, 20, "\x1b]7;file://mac.local/Users/me/My%20Project\x07");
    assert_eq!(t.cwd.as_deref(), Some("/Users/me/My Project"));
    let t = run(2, 20, "\x1b]7;/tmp/x\x1b\\");
    assert_eq!(t.cwd.as_deref(), Some("/tmp/x"));
}

#[test]
fn osc133_prompt_marks_follow_their_lines_into_scrollback() {
    let mut t = Terminal::new(3, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b]133;A\x07$ one\r\nout\r\nout\r\n\x1b]133;A\x07$ two\r\nout\r\nout\r\n\x1b]133;A\x07$ ");
    assert_eq!(t.prompt_lines(), &[0, 3, 6]);
    assert_eq!(t.grid.scrolled, 4, "three rows showing lines 4, 5 and 6");
    // A prompt drawn twice on the same line (a redraw) is one mark.
    vt.feed(&mut t, b"\r\x1b]133;A\x07$ ");
    assert_eq!(t.prompt_lines(), &[0, 3, 6]);
}

#[test]
fn prompt_marks_are_dropped_with_the_scrollback() {
    let mut t = Terminal::new(2, 20, 2);
    let mut vt = Vt::new();
    for _ in 0..6 {
        vt.feed(&mut t, b"\x1b]133;A\x07$ x\r\n");
    }
    // Only two lines of history survive above the two on screen.
    let oldest = t.grid.scrolled - t.grid.scrollback.len() as u64;
    let scrolled = t.grid.scrolled;
    let marks = t.prompt_lines().to_vec();
    assert!(marks.iter().all(|&l| l >= oldest), "{marks:?} vs oldest {oldest} (scrolled {scrolled})");
    assert_eq!(marks, vec![3, 4, 5]);
}

#[test]
fn the_colour_scheme_is_reported_when_asked() {
    let mut t = run(4, 20, "\x1b[?996n");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?997;1n", "the default theme is dark");
    t.replies.clear();
    t.set_palette(arabic_term::color::Palette::light());
    Vt::new().feed(&mut t, b"\x1b[?996n");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?997;2n");
}

#[test]
fn mode_2031_is_known_and_reported() {
    let t = run(4, 20, "\x1b[?2031$p");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?2031;2$y", "known, and off until asked for");
    let t = run(4, 20, "\x1b[?2031h\x1b[?2031$p");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?2031;1$y");
}

#[test]
fn a_theme_change_is_announced_only_to_programs_that_asked() {
    use arabic_term::color::Palette;
    let mut t = run(4, 20, "");
    t.set_palette(Palette::light());
    assert!(t.replies.is_empty(), "nothing unasked without mode 2031");

    Vt::new().feed(&mut t, b"\x1b[?2031h");
    t.set_palette(Palette::dark());
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?997;1n");
    t.replies.clear();
    t.set_palette(Palette::light());
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?997;2n");

    t.replies.clear();
    Vt::new().feed(&mut t, b"\x1b[?2031l");
    t.set_palette(Palette::dark());
    assert!(t.replies.is_empty(), "and nothing once it is switched off");
}
