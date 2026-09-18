//! Font stack: coverage and fallback.

use arabic_term::font::{Fonts, Slot, Style};

#[test]
fn a_monospace_face_is_always_found() {
    let fonts = Fonts::new(16.0, None).expect("some monospace font must be available");
    assert!(fonts.cell_w > 0.0);
    assert!(fonts.cell_h > fonts.cell_w, "a text cell is taller than it is wide");
    assert!(fonts.baseline > 0.0 && fonts.baseline < fonts.cell_h);
}

#[test]
fn ascii_and_box_drawing_resolve_to_the_mono_face() {
    let fonts = Fonts::new(16.0, None).unwrap();
    for c in ['a', 'Z', '0', '$', '|', '-'] {
        assert_eq!(fonts.slot_for(c, Style::default()), Slot::Mono, "{c:?}");
    }
}

#[test]
fn arabic_routes_to_the_arabic_face() {
    let fonts = Fonts::new(16.0, None).unwrap();
    for c in ['م', 'ا', 'ب', '\u{0651}', '\u{0664}'] {
        assert_eq!(fonts.slot_for(c, Style::default()), Slot::Arabic, "{c:?}");
    }
}

#[test]
fn nerd_font_icons_have_a_covering_face() {
    // Status bars, file trees and workspace managers draw with private-use
    // icons. Without a face that covers them they render as .notdef boxes.
    let mut fonts = Fonts::new(16.0, None).unwrap();
    // A folder, a branch, and the Apple logo: common Nerd Font codepoints.
    let icons = ['\u{e5ff}', '\u{e0a0}', '\u{f179}'];
    let covered: Vec<char> = icons
        .into_iter()
        .filter(|&c| {
            let slot = fonts.slot_for(c, Style::default());
            let id = fonts.face_id(slot, Style::default());
            fonts.faces[id].covers(c)
        })
        .collect();

    if covered.is_empty() {
        eprintln!("no Nerd Font installed; icon coverage not testable here");
        return;
    }
    assert_eq!(covered.len(), icons.len(), "some icons resolved to a face without them");

    // And they must actually rasterize to something with ink.
    for c in icons {
        let slot = fonts.slot_for(c, Style::default());
        let id = fonts.face_id(slot, Style::default());
        let gid = fonts.faces[id].hb.glyph_index(c).expect("glyph id").0;
        let px = fonts.px_for(slot);
        let g = fonts.rasterize(id, gid, px, 0.0);
        assert!(g.is_some(), "icon {c:?} rasterized to nothing");
    }
}

#[test]
fn bold_and_italic_resolve_to_some_face() {
    let fonts = Fonts::new(16.0, None).unwrap();
    for style in [
        Style { bold: true, italic: false },
        Style { bold: false, italic: true },
        Style { bold: true, italic: true },
    ] {
        let id = fonts.face_id(Slot::Mono, style);
        assert!(fonts.faces[id].covers('a'), "no usable face for {style:?}");
    }
}

#[test]
fn line_height_adds_leading_without_resizing_glyphs_and_survives_zoom() {
    let mut fonts = Fonts::new(16.0, None).unwrap();
    let (width, height, baseline, scale, generation) =
        (fonts.cell_w, fonts.cell_h, fonts.baseline, fonts.arabic_scale, fonts.generation());
    fonts.set_line_height(1.2);
    assert_eq!(fonts.cell_w, width);
    assert_eq!(fonts.arabic_scale, scale);
    assert!(fonts.cell_h > height);
    assert!((fonts.baseline - baseline - (fonts.cell_h - height) * 0.5).abs() <= 1.0);
    assert!(fonts.generation() > generation);
    fonts.set_size(32.0);
    let natural = Fonts::new(32.0, None).unwrap();
    assert!(fonts.cell_h > natural.cell_h);
    assert_eq!(fonts.cell_w, natural.cell_w);
}

#[test]
fn changing_fonts_reuses_the_loaded_file_bytes() {
    let a = Fonts::with_families(16.0, None, None, Some("Amiri")).unwrap();
    let b = Fonts::with_families(18.0, None, None, Some("Amiri")).unwrap();
    let fa = &a.faces[a.face_id(Slot::Arabic, Style::default())];
    let fb = &b.faces[b.face_id(Slot::Arabic, Style::default())];
    assert!(std::ptr::eq(fa.data, fb.data), "font switches must not leak another file copy");
}

#[test]
fn installed_arabic_presets_preserve_marks_and_cell_mapping() {
    use arabic_term::{font::ArabicFont, layout::Layouter, term::{Terminal, Vt}};
    for choice in [ArabicFont::Naskh, ArabicFont::Amiri, ArabicFont::Scheherazade, ArabicFont::Geeza] {
        let mut fonts = Fonts::with_families(32.0, None, None, Some(choice.family())).unwrap();
        if fonts.arabic_family != choice.family() { continue; }
        for style in ["", "\x1b[1m"] {
            let mut term = Terminal::new(2, 60, 0);
            let text = format!("{style}بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ README");
            Vt::new().feed(&mut term, text.as_bytes());
            let row = term.grid.viewport_row(0);
            let layout = Layouter::new().layout_row(row, 60, &mut fonts, term.bidi_mode, true);
            assert!(!layout.glyphs.is_empty());
            for glyph in &layout.glyphs {
                assert_ne!(glyph.glyph, 0, "{}: missing shaped glyph", choice.family());
                let y = fonts.baseline - glyph.y;
                let h = fonts.cell_h;
                if let Some(raster) = fonts.rasterize(glyph.font, glyph.glyph, glyph.px, glyph.x) {
                    let top = y.floor() - raster.top as f32;
                    assert!(top >= -1.0 && top + raster.height as f32 <= h + 1.0,
                        "{}: marks clip the row ({top}, {}, {h})", choice.family(), raster.height);
                }
            }
            for cell in 0..60 {
                assert_eq!(layout.visual_cells[layout.column_of(cell)], cell, "{}: cursor map", choice.family());
            }
        }
    }
}

#[test]
fn editor_hamza_forms_shape_like_normal_arabic_in_every_preset() {
    use arabic_term::{font::ArabicFont, layout::Layouter, term::{Terminal, Vt}};
    let editor = "\"title\": \"\u{FE88}نشاء الر\u{FE8C}يسية الم\u{FE86}شرات \u{FE84}فضل \u{FE82}فاق README\"";
    let plain = "\"title\": \"إنشاء الرئيسية المؤشرات أفضل آفاق README\"";
    for choice in [ArabicFont::Naskh, ArabicFont::Amiri, ArabicFont::Scheherazade, ArabicFont::Geeza] {
        let mut fonts = Fonts::with_families(32.0, None, None, Some(choice.family())).unwrap();
        if fonts.arabic_family != choice.family() { continue; }
        for style in ["", "\x1b[1m", "\x1b[3m", "\x1b[1;3m"] {
            let layouts: Vec<_> = [editor, plain].into_iter().map(|text| {
                let mut term = Terminal::new(2, 80, 0);
                Vt::new().feed(&mut term, format!("{style}{text}").as_bytes());
                Layouter::new().layout_row(term.grid.viewport_row(0), 80, &mut fonts, term.bidi_mode, true)
            }).collect();
            let (actual, expected) = (&layouts[0], &layouts[1]);
            assert_eq!(actual.visual_cells, expected.visual_cells, "{}: visual columns", choice.family());
            assert_eq!(actual.cell_to_column, expected.cell_to_column, "{}: cursor columns", choice.family());
            let glyphs = |layout: &arabic_term::layout::RowLayout| layout.glyphs.iter()
                .map(|g| (g.font, g.glyph, g.x, g.y, g.px, g.cell)).collect::<Vec<_>>();
            // Compare contextual shapes and placement as well as glyph coverage.
            assert_eq!(glyphs(actual), glyphs(expected), "{} {style:?}: editor shaping", choice.family());
            assert!(actual.glyphs.iter().all(|g| g.glyph != 0), "{}: missing glyph", choice.family());
        }
    }
}

#[test]
fn noto_variable_bold_has_the_correct_shaping_and_raster_weight() {
    let mut fonts = Fonts::with_families(32.0, None, None, Some("Noto Naskh Arabic")).unwrap();
    if fonts.arabic_family != "Noto Naskh Arabic" { return; }
    let regular = fonts.face_id(Slot::Arabic, Style::default());
    let bold = fonts.face_id(Slot::Arabic, Style { bold: true, italic: false });
    if fonts.faces[regular].weight.is_none() { return; }
    assert_eq!(fonts.faces[regular].weight, Some(400.0));
    assert_eq!(fonts.faces[bold].weight, Some(700.0));
    assert_ne!(fonts.faces[regular].hb.variation_coordinates(), fonts.faces[bold].hb.variation_coordinates());
    let glyph = fonts.faces[regular].hb.glyph_index('م').unwrap().0;
    let px = fonts.px_for(Slot::Arabic);
    let regular_ink: u64 = fonts.rasterize(regular, glyph, px, 0.0).unwrap().data.iter().map(|&n| n as u64).sum();
    let bold_ink: u64 = fonts.rasterize(bold, glyph, px, 0.0).unwrap().data.iter().map(|&n| n as u64).sum();
    assert!(bold_ink > regular_ink, "bold must render heavier, not only shape differently");
}
