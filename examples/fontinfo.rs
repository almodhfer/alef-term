//! Report which faces the font stack actually resolved to.
use arabic_term::font::{Fonts, Slot, Style};
fn main() -> anyhow::Result<()> {
    let fonts = Fonts::new(16.0, None)?;
    println!("cell {}x{}  baseline {}  arabic_scale {:.3}",
        fonts.cell_w, fonts.cell_h, fonts.baseline, fonts.arabic_scale);
    for slot in [Slot::Mono, Slot::Arabic, Slot::Symbols, Slot::Emoji, Slot::Fallback] {
        for style in [Style::default(), Style { bold: true, italic: false },
                      Style { bold: false, italic: true }] {
            let id = fonts.face_id(slot, style);
            let f = &fonts.faces[id];
            println!("{slot:?}/{}{} -> face#{id} upem={} covers(م)={} covers(a)={} covers(icon)={}",
                if style.bold {"b"} else {"-"}, if style.italic {"i"} else {"-"},
                f.upem, f.covers('م'), f.covers('a'), f.covers('\u{e0a0}'));
        }
    }
    Ok(())
}
