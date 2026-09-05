//! Encoding mouse events for applications that ask to receive them.
//!
//! Kept free of window state so it can be tested directly — the omission this
//! module exists to prevent (the wheel never being reported at all) survived
//! precisely because there was nothing to test.

use winit::keyboard::ModifiersState;

/// Button numbers as the protocol defines them.
pub const BTN_LEFT: u8 = 0;
pub const BTN_MIDDLE: u8 = 1;
pub const BTN_RIGHT: u8 = 2;
/// The wheel reports as buttons 64 and 65, and only ever as a press.
pub const BTN_WHEEL_UP: u8 = 64;
pub const BTN_WHEEL_DOWN: u8 = 65;

#[inline]
pub fn is_wheel(button: u8) -> bool {
    button == BTN_WHEEL_UP || button == BTN_WHEEL_DOWN
}

/// Modifier bits folded into the button number: shift 4, meta 8, control 16.
fn with_modifiers(button: u8, mods: ModifiersState) -> u8 {
    button
        + if mods.shift_key() { 4 } else { 0 }
        + if mods.alt_key() { 8 } else { 0 }
        + if mods.control_key() { 16 } else { 0 }
}

/// Motion is reported with 32 added to the button number.
pub const MOTION: u8 = 32;
/// Motion with no button held, for any-motion tracking (mode 1003).
pub const BTN_NONE: u8 = 3;

/// Encode one event, or `None` when it cannot be expressed.
///
/// `row` and `col` are zero-based; the wire format is one-based. `sgr` selects
/// the 1006 encoding over the original X10 one.
pub fn encode(
    button: u8,
    pressed: bool,
    row: usize,
    col: usize,
    mods: ModifiersState,
    sgr: bool,
) -> Option<Vec<u8>> {
    let b = with_modifiers(button, mods);
    let (r, c) = (row + 1, col + 1);

    if sgr {
        // SGR can express any coordinate, and distinguishes release by the
        // final byte rather than by a magic button number.
        let final_byte = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{b};{c};{r}{final_byte}").into_bytes());
    }

    // X10 has one byte per field, so anything past 223 is inexpressible. Drop
    // the report rather than clamping: clamping would tell the application the
    // pointer is somewhere it is not, which is worse than silence.
    if c > 223 || r > 223 {
        return None;
    }

    // X10: one byte per field, offset by 32. Release is button 3, except for
    // the wheel, which has no release to report.
    let code = if pressed || is_wheel(button) { b } else { with_modifiers(3, mods) };
    let mut out = Vec::with_capacity(6);
    out.extend_from_slice(b"\x1b[M");
    out.push(32u8.saturating_add(code));
    out.push(32 + c as u8);
    out.push(32 + r as u8);
    Some(out)
}
