//! Mixed-direction regressions: the English word, punctuation and caret must
//! agree with the visual cell map while the terminal stores logical text.
use arabic_term::font::Fonts;
use arabic_term::layout::Layouter;
use arabic_term::term::{BidiMode, Terminal, Vt};

fn visual(text: &str, mode: BidiMode) -> String {
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let mut term = Terminal::new(1, 100, 0);
    Vt::new().feed(&mut term, text.as_bytes());
    let row = &term.grid.screen.rows[0];
    let layout = Layouter::new().layout_row(row, 100, &mut fonts, mode, true);
    for cell in 0..100 {
        assert_eq!(layout.cell_at(layout.column_of(cell)), cell);
    }
    (0..100).map(|col| row.cells[layout.cell_at(col)].ch).collect::<String>().trim_end().to_string()
}

#[test]
fn arabic_sentence_with_one_english_word_keeps_sentence_order_by_default() {
    assert_eq!(visual("افتح الملف README ثم احفظ التغييرات", BidiMode::default()),
        "تارييغتلا ظفحا مث README فلملا حتفا");
}

#[test]
fn english_sentences_and_shell_prompts_keep_their_frame() {
    for text in ["$ مرحبا README", "% مرحبا README", "❯ مرحبا README", "> مرحبا README",
                 "~/مشاريع $ echo مرحبا", "Read مرحبا in README"] {
        assert_eq!(visual(text, BidiMode::Auto), visual(text, BidiMode::ForceLtr), "{text}");
    }
}

#[test]
fn automatic_direction_preserves_table_columns_and_latin_numbers() {
    assert_eq!(visual("│افتح README الآن│  123.45  │abc│", BidiMode::Auto),
        "│نآلا README حتفا│  123.45  │abc│");
}

#[test]
fn rtl_punctuation_glyphs_stay_centered_on_their_own_cells() {
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let mut term = Terminal::new(1, 40, 0);
    Vt::new().feed(&mut term, "مرحبا...بالعالم".as_bytes());
    let row = &term.grid.screen.rows[0];
    let layout = Layouter::new().layout_row(row, 40, &mut fonts, BidiMode::ForceLtr, true);
    let offsets: Vec<f32> = layout.glyphs.iter().filter(|g| row.cells[g.cell].ch == '.')
        .map(|g| g.x - layout.column_of(g.cell) as f32 * fonts.cell_w).collect();
    assert_eq!(offsets.len(), 3);
    for offset in &offsets {
        assert!((offset - offsets[0]).abs() < 0.01, "punctuation drifted: {offsets:?}");
    }
}

#[test]
fn cursor_direction_is_local_to_the_character() {
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let mut term = Terminal::new(1, 40, 0);
    let text = "مرحبا README عالم";
    Vt::new().feed(&mut term, text.as_bytes());
    let layout = Layouter::new().layout_row(&term.grid.screen.rows[0], 40,
        &mut fonts, BidiMode::Auto, true);
    assert!(layout.is_rtl(0));
    for cell in 6..12 {
        assert!(!layout.is_rtl(cell), "English cell {cell} must have an LTR caret");
    }
    assert!(layout.is_rtl(13));
    assert!(!layout.is_rtl(39), "empty cells keep the terminal's logical cursor");
}

#[test]
fn wide_characters_round_trip_in_reordered_fields() {
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let mut term = Terminal::new(1, 40, 0);
    Vt::new().feed(&mut term, "مرحبا 😀 README عالم".as_bytes());
    let layout = Layouter::new().layout_row(&term.grid.screen.rows[0], 40,
        &mut fonts, BidiMode::Auto, true);
    for cell in 0..40 {
        assert_eq!(layout.cell_at(layout.column_of(cell)), cell, "cell {cell}");
    }
}

#[test]
fn file_extensions_and_paths_keep_their_terminal_order() {
    for text in ["تقرير.pdf", "مجلد/README.md", "~/مشاريع", "./برنامج"] {
        assert_eq!(visual(text, BidiMode::Auto), visual(text, BidiMode::ForceLtr), "{text}");
    }
}

#[test]
fn changing_direction_preserves_logical_copy_order() {
    use arabic_term::selection::{selected_text, Selection};
    let mut term = Terminal::new(1, 80, 0);
    let text = "افتح الملف README ثم احفظ التغييرات";
    Vt::new().feed(&mut term, text.as_bytes());
    term.bidi_mode = BidiMode::Auto;
    assert_eq!(selected_text(&term, Selection {
        start: (0, 0), end: (0, text.chars().count() - 1), block: false,
    }), text);
}

#[test]
fn terminal_reset_keeps_the_users_direction_preference() {
    let mut term = Terminal::new(1, 20, 0);
    term.bidi_mode = BidiMode::ForceLtr;
    Vt::new().feed(&mut term, b"\x1bc");
    assert_eq!(term.bidi_mode, BidiMode::ForceLtr);
}
