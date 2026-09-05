//! Mouse event encoding — the path that had no coverage at all, which is how
//! the wheel came to be missing from it.

use arabic_term::mouse::{encode, is_wheel, BTN_LEFT, BTN_RIGHT, BTN_WHEEL_DOWN, BTN_WHEEL_UP};
use winit::keyboard::ModifiersState;

fn none() -> ModifiersState {
    ModifiersState::empty()
}

#[test]
fn sgr_encodes_a_left_click_and_its_release() {
    // Coordinates are one-based on the wire.
    assert_eq!(encode(BTN_LEFT, true, 4, 9, none(), true), b"\x1b[<0;10;5M");
    assert_eq!(encode(BTN_LEFT, false, 4, 9, none(), true), b"\x1b[<0;10;5m");
}

#[test]
fn legacy_encodes_with_a_32_offset_and_button_3_for_release() {
    // ESC [ M then button+32, col+32, row+32.
    assert_eq!(encode(BTN_LEFT, true, 0, 0, none(), false), b"\x1b[M\x20\x21\x21");
    // Release is button 3 in the legacy encoding, which has no button field.
    assert_eq!(encode(BTN_RIGHT, false, 0, 0, none(), false), b"\x1b[M\x23\x21\x21");
}

#[test]
fn the_wheel_reports_as_buttons_64_and_65() {
    assert!(is_wheel(BTN_WHEEL_UP) && is_wheel(BTN_WHEEL_DOWN));
    assert!(!is_wheel(BTN_LEFT));
    assert_eq!(encode(BTN_WHEEL_UP, true, 0, 0, none(), true), b"\x1b[<64;1;1M");
    assert_eq!(encode(BTN_WHEEL_DOWN, true, 0, 0, none(), true), b"\x1b[<65;1;1M");
}

#[test]
fn the_wheel_never_reports_a_release() {
    // Legacy release normally collapses to button 3; the wheel must not, or
    // an application sees a phantom button-3 click on every scroll.
    let up = encode(BTN_WHEEL_UP, false, 0, 0, none(), false);
    assert_eq!(up, b"\x1b[M\x60\x21\x21", "wheel keeps its own button number");
    assert_eq!(up[3], 32 + 64, "64 + 32 = 96");
}

#[test]
fn modifiers_are_folded_into_the_button_number() {
    // shift 4, alt 8, ctrl 16.
    assert_eq!(encode(BTN_LEFT, true, 0, 0, ModifiersState::SHIFT, true), b"\x1b[<4;1;1M");
    assert_eq!(encode(BTN_LEFT, true, 0, 0, ModifiersState::ALT, true), b"\x1b[<8;1;1M");
    assert_eq!(encode(BTN_LEFT, true, 0, 0, ModifiersState::CONTROL, true), b"\x1b[<16;1;1M");
    let both = ModifiersState::SHIFT | ModifiersState::CONTROL;
    assert_eq!(encode(BTN_LEFT, true, 0, 0, both, true), b"\x1b[<20;1;1M");
}

#[test]
fn sgr_expresses_coordinates_the_legacy_encoding_cannot() {
    // Legacy has one byte per field and clamps; SGR is unbounded.
    let big = encode(BTN_LEFT, true, 400, 500, none(), true);
    assert_eq!(big, b"\x1b[<0;501;401M");
    let legacy = encode(BTN_LEFT, true, 400, 500, none(), false);
    assert_eq!(legacy[4], 32 + 223, "column clamped rather than wrapped");
    assert_eq!(legacy[5], 32 + 223, "row clamped rather than wrapped");
}
