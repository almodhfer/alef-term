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
