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
