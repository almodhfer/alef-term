//! The core of the project: bidi ordering, Arabic joining, and column pinning.

use arabic_term::font::Fonts;
use arabic_term::layout::Layouter;
use arabic_term::term::{BidiMode, Terminal, Vt};

/// Lay out a single row of text and return the layout plus the terminal.
fn layout(text: &str, cols: usize, mode: BidiMode) -> (arabic_term::layout::RowLayout, Terminal) {
    let mut fonts = Fonts::new(16.0, None).expect("system fonts");
    let mut t = Terminal::new(3, cols, 10);
    let mut vt = Vt::new();
    vt.feed(&mut t, text.as_bytes());
    let mut l = Layouter::new();
    let out = l.layout_row(&t.grid.screen.rows[0], cols, &mut fonts, mode, true);
    (out, t)
}

/// The characters as they appear left-to-right on screen.
fn visual_string(l: &arabic_term::layout::RowLayout, t: &Terminal, cols: usize) -> String {
    let row = &t.grid.screen.rows[0];
    let mut s = String::new();
    let mut last = usize::MAX;
    for c in 0..cols {
        let cell = l.cell_at(c);
        if cell == last {
            continue;
        }
        last = cell;
        if let Some(x) = row.cells.get(cell) {
            s.push(x.ch);
        }
    }
    s.trim_end().to_string()
}

#[test]
fn ltr_text_is_not_reordered() {
    let cols = 20;
    let (l, t) = layout("hello world", cols, BidiMode::ForceLtr);
    assert_eq!(visual_string(&l, &t, cols), "hello world");
}

#[test]
fn arabic_word_is_reversed_onto_columns() {
    // Logical م-ر-ح-ب-ا must appear on screen as ا-ب-ح-ر-م (right to left).
    let cols = 20;
    let (l, t) = layout("مرحبا", cols, BidiMode::ForceLtr);
    assert_eq!(visual_string(&l, &t, cols), "ابحرم");
    // And the mapping must be a bijection over the occupied columns.
    assert_eq!(l.cell_at(0), 4, "leftmost column shows the last logical letter");
    assert_eq!(l.cell_at(4), 0);
    assert_eq!(l.column_of(0), 4, "first logical letter sits at the rightmost column");
}

#[test]
fn logical_and_visual_maps_are_inverse() {
    let cols = 32;
    let (l, _t) = layout("ab مرحبا cd", cols, BidiMode::ForceLtr);
    for cell in 0..11 {
        let col = l.column_of(cell);
        assert_eq!(l.cell_at(col), cell, "cell {cell} round-trips through column {col}");
    }
}

#[test]
fn arabic_letters_join_into_fewer_glyphs_via_ligatures() {
    let cols = 24;
    // لا is a mandatory lam-alef ligature: two cells, one glyph.
    let (l, _t) = layout("لا", cols, BidiMode::ForceLtr);
    assert_eq!(l.glyphs.len(), 1, "lam-alef must form a single ligature glyph");
    // It still occupies both logical columns.
    assert_eq!(l.cell_at(0), 1);
    assert_eq!(l.cell_at(1), 0);
}

#[test]
fn blank_columns_emit_no_glyphs() {
    // A mostly-empty row should not pay for a glyph per blank column.
    let cols = 40;
    let (l, _t) = layout("hi", cols, BidiMode::ForceLtr);
    assert_eq!(l.glyphs.len(), 2, "only the two visible characters produce glyphs");
}

#[test]
fn harakat_add_glyphs_without_adding_columns() {
    let cols = 24;
    let (plain, _) = layout("بسم", cols, BidiMode::ForceLtr);
    let (marked, t) = layout("بِسْمِ", cols, BidiMode::ForceLtr);
    assert!(
        marked.glyphs.len() > plain.glyphs.len(),
        "diacritics must produce extra mark glyphs ({} vs {})",
        marked.glyphs.len(),
        plain.glyphs.len()
    );
    assert_eq!(t.grid.screen.cursor.col, 3, "still only three columns wide");
}

fn paren_gids() -> (u16, u16) {
    let fonts = Fonts::new(16.0, None).unwrap();
    let mono = fonts.face_id(arabic_term::font::Slot::Mono, Default::default());
    let face = &fonts.faces[mono];
    (face.hb.glyph_index('(').unwrap().0, face.hb.glyph_index(')').unwrap().0)
}

#[test]
fn brackets_are_mirrored_exactly_once_in_an_rtl_run() {
    // rustybuzz applies UAX#9 L4 for right-to-left buffers. Mirroring by hand
    // as well would cancel it back out, so assert the swap happened once.
    let (open, close) = paren_gids();
    assert_ne!(open, close);

    let cols = 20;
    // Leading Arabic makes the bracket pair resolve RTL.
    let (l, _t) = layout("قال (نعم)", cols, BidiMode::ForceLtr);
    let g = l.glyphs.iter().find(|g| g.cell == 4).expect("glyph for '('");
    assert_eq!(g.glyph, close, "logical '(' in an RTL run renders as ')'");
    let g2 = l.glyphs.iter().find(|g| g.cell == 8).expect("glyph for ')'");
    assert_eq!(g2.glyph, open, "logical ')' in an RTL run renders as '('");
    // And the mirrored opening bracket sits to the RIGHT of the closing one.
    assert!(g.x > g2.x, "the opening bracket is on the right in RTL");
}

#[test]
fn brackets_are_not_mirrored_when_rule_n0_keeps_them_ltr() {
    // UAX#9 N0: a bracket pair enclosing RTL text inside an LTR paragraph,
    // with no preceding RTL context, takes the embedding direction. The
    // brackets then stay LTR and must NOT be mirrored.
    let (open, close) = paren_gids();
    let cols = 20;
    let (l, _t) = layout("(نعم)", cols, BidiMode::ForceLtr);
    let g = l.glyphs.iter().find(|g| g.cell == 0).expect("glyph for '('");
    assert_eq!(g.glyph, open, "'(' stays '(' when the pair resolves LTR");
    let g2 = l.glyphs.iter().find(|g| g.cell == 4).expect("glyph for ')'");
    assert_eq!(g2.glyph, close);
    assert!(g.x < g2.x, "the opening bracket stays on the left");
}

#[test]
fn box_drawing_pins_columns_against_reordering() {
    // A table separator must stay where the program drew it even though the
    // fields around it are Arabic. Without segmentation, UAX#9 rule N1 folds
    // the separator into the surrounding RTL run and the columns swap.
    let cols = 24;
    let text = "│اسم│حجم│";
    let (l, t) = layout(text, cols, BidiMode::ForceLtr);
    let row = &t.grid.screen.rows[0];
    let at = |c: usize| row.cells[l.cell_at(c)].ch;
    assert_eq!(at(0), '│', "separator stays at column 0");
    assert_eq!(at(4), '│', "separator stays at column 4");
    assert_eq!(at(8), '│', "separator stays at column 8");
    // Field contents reorder within their own columns.
    assert_eq!(visual_string(&l, &t, cols), "│مسا│مجح│");
}

#[test]
fn double_blank_runs_pin_columns() {
    // Column-aligned output (ls, ps, tables padded with spaces) must not have
    // its columns dragged around by an Arabic field.
    let cols = 32;
    let (l, t) = layout("مرحبا  world", cols, BidiMode::ForceLtr);
    let s = visual_string(&l, &t, cols);
    assert!(s.starts_with("ابحرم"), "Arabic field reorders in place: {s}");
    assert!(s.ends_with("world"), "the LTR field keeps its columns: {s}");
}

#[test]
fn mixed_direction_line_keeps_latin_in_order() {
    let cols = 40;
    let (l, t) = layout("cd ~/مشاريع ok", cols, BidiMode::ForceLtr);
    let s = visual_string(&l, &t, cols);
    assert!(s.starts_with("cd ~/"), "leading LTR run stays put: {s}");
    assert!(s.ends_with(" ok"), "trailing LTR run stays put: {s}");
    assert!(s.contains("عيراشم"), "the Arabic run is reversed in place: {s}");
}

#[test]
fn force_rtl_moves_the_base_direction() {
    let cols = 20;
    let (ltr, t1) = layout("abc", cols, BidiMode::ForceLtr);
    let (rtl, t2) = layout("abc", cols, BidiMode::ForceRtl);
    // A pure-LTR string reads the same either way; what changes is where the
    // run is placed, not the order of its characters.
    assert_eq!(visual_string(&ltr, &t1, cols), "abc");
    assert_eq!(visual_string(&rtl, &t2, cols), "abc");
}

#[test]
fn every_column_maps_to_some_cell() {
    let cols = 40;
    let (l, _t) = layout("ملف (main.rs) — ٤٢ من 128", cols, BidiMode::ForceLtr);
    assert_eq!(l.visual_cells.len(), cols);
    for c in 0..cols {
        assert!(l.cell_at(c) < cols, "column {c} maps into the row");
    }
}

#[test]
fn glyphs_stay_within_the_row_bounds() {
    let cols = 30;
    let (l, _t) = layout("مرحبا بالعالم", cols, BidiMode::ForceLtr);
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let width = cols as f32 * fonts.cell_w;
    for g in &l.glyphs {
        assert!(g.x >= -2.0, "glyph starts before the row: {}", g.x);
        assert!(g.x <= width, "glyph starts past the row end: {} > {width}", g.x);
    }
}

#[test]
fn long_latin_runs_stay_on_the_cell_grid() {
    // `cell_w` is rounded to a whole pixel, so laying a long run out with the
    // font's own advances drifts a fraction of a pixel per character. Across an
    // 80-column status bar that accumulates to a whole cell and characters
    // collide, so non-cursive text is snapped to its cell.
    let cols = 80;
    let text = "F1Help  F2Setup F3SearchF4FilterF5Tree  F6SortByF7Nice -F8Nice +F9Kill  F10Quit";
    let (l, _t) = layout(text, cols, BidiMode::ForceLtr);
    let fonts = Fonts::new(16.0, None).unwrap();
    assert!(!l.glyphs.is_empty());
    for g in &l.glyphs {
        let want = l.column_of(g.cell) as f32 * fonts.cell_w;
        assert!(
            (g.x - want).abs() <= fonts.cell_w * 0.5,
            "glyph for cell {} drifted off its column: x={:.2} want~{:.2}",
            g.cell,
            g.x,
            want
        );
    }
}

#[test]
fn arabic_is_not_snapped_so_letters_can_join() {
    // The converse of the rule above: snapping each Arabic letter to its own
    // cell is what tears cursive joins apart, so a word is placed as one piece
    // and its glyphs sit closer together than the cell pitch.
    let cols = 24;
    let (l, _t) = layout("بالعالم", cols, BidiMode::ForceLtr);
    let fonts = Fonts::new(16.0, None).unwrap();
    let mut xs: Vec<f32> = l.glyphs.iter().map(|g| g.x).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(xs.len() >= 4, "expected several glyphs, got {}", xs.len());
    let tight = xs.windows(2).any(|w| w[1] - w[0] < fonts.cell_w * 0.9);
    assert!(tight, "Arabic glyphs are on the cell pitch; joins would be broken");
}

/// Lay out a row with explicit joining context, as the renderer does across a
/// soft wrap.
fn layout_ctx(
    text: &str,
    cols: usize,
    pre: &str,
    post: &str,
) -> arabic_term::layout::RowLayout {
    use arabic_term::layout::RowContext;
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let mut t = Terminal::new(3, cols, 10);
    let mut vt = Vt::new();
    vt.feed(&mut t, text.as_bytes());
    let mut l = Layouter::new();
    l.layout_row_ctx(
        &t.grid.screen.rows[0],
        cols,
        &mut fonts,
        BidiMode::ForceLtr,
        true,
        RowContext { pre, post },
    )
}

#[test]
fn joining_survives_a_soft_wrap() {
    // "بالعالم" broken by the right margin into "بالع" + "الم". The ع at the
    // end of the first row must take its MEDIAL form, because the word did not
    // end — only the display line did.
    let no_ctx = layout_ctx("بالع", 12, "", "");
    let wrapped = layout_ctx("بالع", 12, "", "الم");

    let last_of = |l: &arabic_term::layout::RowLayout| {
        // Cell 3 is the ع; find the glyph drawn for it.
        l.glyphs.iter().find(|g| g.cell == 3).map(|g| g.glyph)
    };
    let isolated_end = last_of(&no_ctx).expect("glyph for ع");
    let joined_end = last_of(&wrapped).expect("glyph for ع");
    assert_ne!(
        isolated_end, joined_end,
        "ع should take a different (medial) form when the word continues on the next row"
    );
}

#[test]
fn joining_context_applies_from_the_previous_row_too() {
    // The other side of the same break: "الم" continues a word, so its ا must
    // not be shaped as a word-initial form.
    let alone = layout_ctx("الم", 12, "", "");
    let continued = layout_ctx("الم", 12, "بالع", "");
    let first_of = |l: &arabic_term::layout::RowLayout| {
        l.glyphs.iter().find(|g| g.cell == 0).map(|g| g.glyph)
    };
    assert_ne!(
        first_of(&alone).expect("glyph"),
        first_of(&continued).expect("glyph"),
        "ا should join backwards to the previous row"
    );
}

#[test]
fn programming_ligatures_form_and_stay_cell_aligned() {
    // Good terminal fonts implement `->` as a monospace-preserving ligature:
    // each character becomes half of the arrow, so the glyphs change but the
    // cell count does not. That is exactly what a grid needs.
    let fonts = Fonts::new(16.0, None).unwrap();
    let id = fonts.face_id(arabic_term::font::Slot::Mono, Default::default());
    let face = &fonts.faces[id];
    let isolated: Vec<u16> =
        "->".chars().map(|c| face.hb.glyph_index(c).map(|g| g.0).unwrap_or(0)).collect();

    let cols = 20;
    let (l, _t) = layout("a -> b", cols, BidiMode::ForceLtr);
    let shaped: Vec<u16> = l
        .glyphs
        .iter()
        .filter(|g| g.cell == 2 || g.cell == 3)
        .map(|g| g.glyph)
        .collect();
    assert_eq!(shaped.len(), 2, "the arrow keeps one glyph per cell");

    if shaped != isolated {
        // The font ligates: check the halves still sit on their own columns.
        let xs: Vec<f32> = l.glyphs.iter().filter(|g| g.cell == 2 || g.cell == 3).map(|g| g.x).collect();
        assert!(
            (xs[1] - xs[0] - fonts.cell_w).abs() < 1.0,
            "ligature halves must stay one cell apart, got {xs:?}"
        );
    } else {
        eprintln!("font has no ligature for '->'; alignment check skipped");
    }
}
