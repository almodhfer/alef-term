//! Selection and copy, including across reordered Arabic and soft wraps.

use arabic_term::font::Fonts;
use arabic_term::layout::Layouter;
use arabic_term::selection::{selected_text, Selection};
use arabic_term::term::{BidiMode, Terminal, Vt};

fn term(rows: usize, cols: usize, input: &str) -> Terminal {
    let mut t = Terminal::new(rows, cols, 100);
    Vt::new().feed(&mut t, input.as_bytes());
    t
}

fn sel(a: (usize, usize), b: (usize, usize)) -> Selection {
    Selection { start: a, end: b, block: false }
}

#[test]
fn selects_a_span_of_latin() {
    let t = term(3, 20, "hello world");
    assert_eq!(selected_text(&t, sel((0, 0), (0, 4))), "hello");
    assert_eq!(selected_text(&t, sel((0, 6), (0, 10))), "world");
}

#[test]
fn selection_is_direction_agnostic() {
    let t = term(3, 20, "hello");
    // Dragging right-to-left selects the same text.
    assert_eq!(selected_text(&t, sel((0, 4), (0, 0))), "hello");
}

#[test]
fn arabic_copies_in_logical_order() {
    // The screen shows ابحرم; what gets copied must be مرحبا — the order the
    // program wrote, which is what a shell will accept back.
    let t = term(3, 20, "مرحبا");
    assert_eq!(selected_text(&t, sel((0, 0), (0, 4))), "مرحبا");
}

#[test]
fn visual_columns_map_back_to_logical_cells_for_selection() {
    // A drag gives visual columns; those must be translated through the row
    // layout before they mean anything as a selection.
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let t = term(3, 20, "مرحبا");
    let mut l = Layouter::new();
    let layout =
        l.layout_row(&t.grid.screen.rows[0], 20, &mut fonts, BidiMode::ForceLtr, true);

    // Visual column 0 is the last logical letter of the word.
    assert_eq!(layout.cell_at(0), 4);
    assert_eq!(layout.cell_at(4), 0);

    // Selecting the whole visual span selects the whole word, logically.
    let (a, b) = (layout.cell_at(0), layout.cell_at(4));
    assert_eq!(selected_text(&t, sel((0, a), (0, b))), "مرحبا");
}

#[test]
fn diacritics_travel_with_their_letter() {
    let t = term(3, 20, "بِسْمِ");
    // Three cells, but the copied text carries all six codepoints.
    let got = selected_text(&t, sel((0, 0), (0, 2)));
    assert_eq!(got, "بِسْمِ");
    assert_eq!(got.chars().count(), 6);
}

#[test]
fn a_soft_wrapped_line_is_rejoined_on_copy() {
    // 10 columns, so this wraps; copying must give back the original command,
    // not the way the screen happened to break it.
    let t = term(3, 10, "echo hello-world");
    assert!(t.grid.screen.rows[0].wrapped, "line should have wrapped");
    let joined = selected_text(&t, sel((0, 0), (1, 9)));
    assert_eq!(joined, "echo hello-world");
}

#[test]
fn separate_lines_keep_their_newline() {
    let t = term(3, 20, "one\r\ntwo");
    assert_eq!(selected_text(&t, sel((0, 0), (1, 2))), "one\ntwo");
}

#[test]
fn trailing_blanks_are_trimmed() {
    let t = term(3, 20, "hi");
    assert_eq!(selected_text(&t, sel((0, 0), (0, 19))), "hi");
}

#[test]
fn block_selection_takes_a_rectangle() {
    let t = term(3, 20, "abcdef\r\nghijkl\r\nmnopqr");
    let s = Selection { start: (0, 1), end: (2, 3), block: true };
    assert_eq!(selected_text(&t, s), "bcd\nhij\nnop");
}

#[test]
fn wide_characters_are_not_duplicated() {
    // A double-width character occupies two cells but is one character.
    let t = term(3, 20, "日本");
    assert_eq!(selected_text(&t, sel((0, 0), (0, 3))), "日本");
}

#[test]
fn a_selection_stays_on_its_text_when_output_arrives() {
    // Selection rows are absolute lines, not viewport rows. Anchored to the
    // viewport, a selection would slide over the text as output pushed lines
    // into scrollback — ending up covering different characters than the ones
    // that were dragged over.
    let mut t = Terminal::new(4, 20, 500);
    let mut vt = Vt::new();
    for i in 1..=6 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    // Find the absolute line holding "line3" and select it.
    let abs = (0..t.grid.total_lines())
        .find(|&i| {
            t.grid
                .line(i as isize)
                .map(|r| r.cells.iter().map(|c| c.ch).collect::<String>().starts_with("line3"))
                .unwrap_or(false)
        })
        .expect("line3 is somewhere");
    let s = sel((abs, 0), (abs, 4));
    assert_eq!(selected_text(&t, s), "line3");

    // Output arrives, pushing more lines into scrollback.
    for i in 7..=15 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    assert_eq!(selected_text(&t, s), "line3", "selection still covers its text");
}

#[test]
fn a_selection_reaches_into_scrollback() {
    // The oldest retained line must be selectable, not just what is on screen.
    let mut t = Terminal::new(3, 20, 500);
    let mut vt = Vt::new();
    for i in 1..=12 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    assert!(t.grid.scrollback.len() > 3, "content retired into scrollback");
    assert_eq!(selected_text(&t, sel((0, 0), (0, 4))), "line1");
}

mod gestures {
    use super::term;
    use arabic_term::selection::{logical_line, url_at, word_at, Span};

    #[test]
    fn double_click_takes_a_word() {
        let t = term(3, 40, "hello wonderful world");
        assert_eq!(word_at(&t, 0, 8), Some(Span { line: 0, start: 6, end: 15 }));
        assert_eq!(word_at(&t, 0, 0), Some(Span { line: 0, start: 0, end: 5 }));
        assert_eq!(word_at(&t, 0, 5), None, "a blank selects nothing");
        assert_eq!(word_at(&t, 0, 30), None);
    }

    #[test]
    fn a_word_stops_at_shell_punctuation_but_keeps_paths() {
        let t = term(3, 40, "ls (src/main.rs) \"x\"");
        assert_eq!(word_at(&t, 0, 6), Some(Span { line: 0, start: 4, end: 15 }));
        assert_eq!(word_at(&t, 0, 18), Some(Span { line: 0, start: 18, end: 19 }));
    }

    #[test]
    fn arabic_words_are_words_too() {
        let t = term(3, 40, "قال مرحبا بالعالم");
        let w = word_at(&t, 0, 5).unwrap();
        assert_eq!((w.start, w.end), (4, 9));
    }

    #[test]
    fn triple_click_takes_the_logical_line_across_soft_wraps() {
        let t = term(6, 10, "0123456789abcdefghij_and\r\nnext");
        assert_eq!(logical_line(&t, 0), (0, 2));
        assert_eq!(logical_line(&t, 1), (0, 2));
        assert_eq!(logical_line(&t, 2), (0, 2));
        assert_eq!(logical_line(&t, 3), (3, 3));
    }

    #[test]
    fn bare_urls_are_found_and_trailing_punctuation_dropped() {
        let t = term(3, 60, "see https://example.com/a/b?q=1. and (http://x.org/p_(1)) now");
        let (span, url) = url_at(&t, 0, 10).unwrap();
        assert_eq!(url, "https://example.com/a/b?q=1");
        assert_eq!((span.start, span.end), (4, 31));
        let (_, url) = url_at(&t, 0, 40).unwrap();
        assert_eq!(url, "http://x.org/p_(1)", "a bracket the URL opened is kept");
        assert!(url_at(&t, 0, 1).is_none(), "plain words are not links");
        assert!(url_at(&t, 0, 32).is_none(), "the dropped full stop is not part of the link");
    }

    #[test]
    fn a_url_broken_by_the_margin_is_followed_across_rows() {
        let t = term(4, 20, "go to https://example.com/some/long/path/here ok");
        // "go to https://exampl" | "e.com/some/long/path" | "/here ok"
        let (span, url) = url_at(&t, 1, 3).unwrap();
        assert_eq!(url, "https://example.com/some/long/path/here");
        assert_eq!((span.line, span.start, span.end), (1, 0, 20));
        let (span, _) = url_at(&t, 2, 2).unwrap();
        assert_eq!((span.line, span.start, span.end), (2, 0, 5));
    }

    #[test]
    fn osc8_links_win_over_the_text() {
        let t = term(3, 40, "\x1b]8;;https://example.com/hidden\x1b\\click here\x1b]8;;\x1b\\ plain");
        let (span, url) = url_at(&t, 0, 3).unwrap();
        assert_eq!(url, "https://example.com/hidden");
        assert_eq!((span.start, span.end), (0, 10));
        assert!(url_at(&t, 0, 12).is_none());
    }
}
