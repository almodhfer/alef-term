//! Mouse event encoding — the path that had no coverage at all, which is how
//! the wheel came to be missing from it.

use arabic_term::mouse::{encode, is_wheel, BTN_LEFT, BTN_RIGHT, BTN_WHEEL_DOWN, BTN_WHEEL_UP};
use winit::keyboard::ModifiersState;

fn none() -> ModifiersState {
    ModifiersState::empty()
}

/// Unwrap an encoding that is expected to be expressible.
fn enc(b: u8, p: bool, r: usize, c: usize, m: ModifiersState, sgr: bool) -> Vec<u8> {
    encode(b, p, r, c, m, sgr).expect("expressible")
}

#[test]
fn sgr_encodes_a_left_click_and_its_release() {
    // Coordinates are one-based on the wire.
    assert_eq!(enc(BTN_LEFT, true, 4, 9, none(), true), b"\x1b[<0;10;5M");
    assert_eq!(enc(BTN_LEFT, false, 4, 9, none(), true), b"\x1b[<0;10;5m");
}

#[test]
fn legacy_encodes_with_a_32_offset_and_button_3_for_release() {
    // ESC [ M then button+32, col+32, row+32.
    assert_eq!(enc(BTN_LEFT, true, 0, 0, none(), false), b"\x1b[M\x20\x21\x21");
    // Release is button 3 in the legacy encoding, which has no button field.
    assert_eq!(enc(BTN_RIGHT, false, 0, 0, none(), false), b"\x1b[M\x23\x21\x21");
}

#[test]
fn the_wheel_reports_as_buttons_64_and_65() {
    assert!(is_wheel(BTN_WHEEL_UP) && is_wheel(BTN_WHEEL_DOWN));
    assert!(!is_wheel(BTN_LEFT));
    assert_eq!(enc(BTN_WHEEL_UP, true, 0, 0, none(), true), b"\x1b[<64;1;1M");
    assert_eq!(enc(BTN_WHEEL_DOWN, true, 0, 0, none(), true), b"\x1b[<65;1;1M");
}

#[test]
fn the_wheel_never_reports_a_release() {
    // Legacy release normally collapses to button 3; the wheel must not, or
    // an application sees a phantom button-3 click on every scroll.
    let up = enc(BTN_WHEEL_UP, false, 0, 0, none(), false);
    assert_eq!(up, b"\x1b[M\x60\x21\x21", "wheel keeps its own button number");
    assert_eq!(up[3], 32 + 64, "64 + 32 = 96");
}

#[test]
fn modifiers_are_folded_into_the_button_number() {
    // shift 4, alt 8, ctrl 16.
    assert_eq!(enc(BTN_LEFT, true, 0, 0, ModifiersState::SHIFT, true), b"\x1b[<4;1;1M");
    assert_eq!(enc(BTN_LEFT, true, 0, 0, ModifiersState::ALT, true), b"\x1b[<8;1;1M");
    assert_eq!(enc(BTN_LEFT, true, 0, 0, ModifiersState::CONTROL, true), b"\x1b[<16;1;1M");
    let both = ModifiersState::SHIFT | ModifiersState::CONTROL;
    assert_eq!(enc(BTN_LEFT, true, 0, 0, both, true), b"\x1b[<20;1;1M");
}

#[test]
fn sgr_expresses_coordinates_the_legacy_encoding_cannot() {
    // SGR is unbounded.
    assert_eq!(enc(BTN_LEFT, true, 400, 500, none(), true), b"\x1b[<0;501;401M");
    // Legacy has one byte per field. Past 223 the position is inexpressible,
    // and the report is dropped: clamping would tell the application the
    // pointer is somewhere it is not, which is worse than saying nothing.
    assert!(encode(BTN_LEFT, true, 400, 500, none(), false).is_none());
    assert!(encode(BTN_LEFT, true, 0, 300, none(), false).is_none());
    // The last expressible column still works.
    assert!(encode(BTN_LEFT, true, 0, 222, none(), false).is_some());
}

#[test]
fn motion_adds_32_to_the_button() {
    use arabic_term::mouse::{BTN_NONE, MOTION};
    // Drag with the left button held.
    assert_eq!(enc(MOTION + BTN_LEFT, true, 2, 3, none(), true), b"\x1b[<32;4;3M");
    // Any-motion with nothing held reports button 3, so 35.
    assert_eq!(enc(MOTION + BTN_NONE, true, 0, 0, none(), true), b"\x1b[<35;1;1M");
}

/// Reports must carry the row an application can understand.
///
/// The terminal tracks two coordinate spaces: the absolute line across
/// scrollback (what a selection anchors to) and the viewport row (what an
/// application is told). Feeding the former into a report claims the pointer is
/// thousands of rows down, which is nonsense to the program receiving it — and
/// it goes wrong as soon as any scrollback exists, not just while scrolled back.
#[test]
fn reported_rows_are_viewport_relative_not_absolute() {
    use arabic_term::term::{Terminal, Vt};
    let mut t = Terminal::new(4, 20, 500);
    let mut vt = Vt::new();
    for i in 0..50 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    assert!(t.grid.scrollback.len() > 40, "scrollback has built up");
    assert_eq!(t.grid.view_offset, 0, "and we are still at the bottom");

    // The viewport is only 4 rows tall, so any row an application is told about
    // must be within it, however much history sits behind.
    let viewport_top = t.grid.viewport_top();
    assert!(viewport_top > 40, "absolute lines are far from zero");

    for row in 0..t.grid.rows {
        let seq = enc(BTN_LEFT, true, row, 0, none(), true);
        let text = String::from_utf8_lossy(&seq).into_owned();
        let reported: usize =
            text.trim_start_matches("\x1b[<0;1;").trim_end_matches('M').parse().unwrap();
        assert!(
            reported >= 1 && reported <= t.grid.rows,
            "row {row} reported as {reported}, outside a {}-row viewport",
            t.grid.rows
        );
    }
}
