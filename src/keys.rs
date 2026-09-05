//! Translating key presses into the bytes a terminal application expects.
//!
//! Kept free of any window or terminal state so it can be tested directly.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Terminal state that changes how a key is encoded.
#[derive(Clone, Copy, Default)]
pub struct KeyContext {
    /// DECCKM: arrows send SS3 (`ESC O A`) instead of CSI (`ESC [ A`).
    pub app_cursor: bool,
}

/// Encode a key press. `text` is the text the OS keyboard layout produced,
/// which is what carries Arabic (and every other non-ASCII layout) through.
pub fn encode(
    key: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    ctx: KeyContext,
) -> Option<Vec<u8>> {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let shift = mods.shift_key();

    // xterm's modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let modifier = 1 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);
    // F1..F4 are SS3-introduced regardless of cursor-key mode, unlike the
    // arrows and Home/End, which switch between CSI and SS3 with DECCKM.
    let ss3 = |last: char| -> Vec<u8> {
        match modifier {
            1 => format!("\x1bO{last}").into_bytes(),
            m => format!("\x1b[1;{m}{last}").into_bytes(),
        }
    };
    let csi = |last: char, num: Option<u8>| -> Vec<u8> {
        match (num, modifier) {
            (Some(n), 1) => format!("\x1b[{n}~").into_bytes(),
            (Some(n), m) => format!("\x1b[{n};{m}~").into_bytes(),
            (None, 1) => {
                let intro = if ctx.app_cursor { 'O' } else { '[' };
                format!("\x1b{intro}{last}").into_bytes()
            }
            (None, m) => format!("\x1b[1;{m}{last}").into_bytes(),
        }
    };

    let out = match key {
        Key::Named(n) => match n {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => {
                if ctrl {
                    b"\x08".to_vec()
                } else {
                    b"\x7f".to_vec()
                }
            }
            NamedKey::Tab => {
                if shift {
                    b"\x1b[Z".to_vec()
                } else {
                    b"\t".to_vec()
                }
            }
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::Space => b" ".to_vec(),
            NamedKey::ArrowUp => csi('A', None),
            NamedKey::ArrowDown => csi('B', None),
            NamedKey::ArrowRight => csi('C', None),
            NamedKey::ArrowLeft => csi('D', None),
            NamedKey::Home => csi('H', None),
            NamedKey::End => csi('F', None),
            NamedKey::Insert => csi(' ', Some(2)),
            NamedKey::Delete => csi(' ', Some(3)),
            NamedKey::PageUp => csi(' ', Some(5)),
            NamedKey::PageDown => csi(' ', Some(6)),
            NamedKey::F1 => ss3('P'),
            NamedKey::F2 => ss3('Q'),
            NamedKey::F3 => ss3('R'),
            NamedKey::F4 => ss3('S'),
            NamedKey::F5 => csi(' ', Some(15)),
            NamedKey::F6 => csi(' ', Some(17)),
            NamedKey::F7 => csi(' ', Some(18)),
            NamedKey::F8 => csi(' ', Some(19)),
            NamedKey::F9 => csi(' ', Some(20)),
            NamedKey::F10 => csi(' ', Some(21)),
            NamedKey::F11 => csi(' ', Some(23)),
            NamedKey::F12 => csi(' ', Some(24)),
            _ => return None,
        },
        Key::Character(s) => {
            if ctrl {
                // Ctrl + letter collapses to the matching C0 control code.
                let c = s.chars().next()?;
                let b = match c {
                    'a'..='z' => (c as u8) - b'a' + 1,
                    'A'..='Z' => (c as u8) - b'A' + 1,
                    '[' => 27,
                    '\\' => 28,
                    ']' => 29,
                    '^' => 30,
                    '_' => 31,
                    ' ' => 0,
                    '?' => 127,
                    _ => return None,
                };
                vec![b]
            } else {
                // The ordinary path: send whatever the keyboard layout produced
                // as UTF-8. Arabic needs no special case — it arrives here like
                // any other text.
                text.unwrap_or(s.as_str()).as_bytes().to_vec()
            }
        }
        _ => match text {
            Some(t) if !t.is_empty() => t.as_bytes().to_vec(),
            _ => return None,
        },
    };

    // Alt acts as Meta: prefix with ESC. Named keys already carry the modifier
    // in their CSI parameter, so they are left alone.
    if alt && !matches!(key, Key::Named(_)) {
        let mut v = Vec::with_capacity(out.len() + 1);
        v.push(0x1b);
        v.extend_from_slice(&out);
        return Some(v);
    }
    Some(out)
}
