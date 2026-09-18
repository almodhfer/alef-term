//! Clipboard and shortcut handling independent of a native window.
use std::path::Path;
use winit::keyboard::{Key, KeyCode, PhysicalKey};

/// Keep Command shortcuts available while typing on an Arabic keyboard.
/// ASCII layouts retain their logical bindings (including Dvorak/AZERTY).
pub fn command_key(logical: &Key, physical: PhysicalKey) -> Key {
    if let Key::Character(c) = logical {
        if c.is_ascii() {
            return Key::Character(c.to_ascii_lowercase().into());
        }
    }
    let PhysicalKey::Code(code) = physical else { return logical.clone() };
    let text = match code {
        KeyCode::KeyA => "a", KeyCode::KeyC => "c", KeyCode::KeyF => "f",
        KeyCode::KeyG => "g", KeyCode::KeyK => "k", KeyCode::KeyN => "n",
        KeyCode::KeyP => "p", KeyCode::KeyQ => "q", KeyCode::KeyR => "r",
        KeyCode::KeyT => "t", KeyCode::KeyV => "v", KeyCode::KeyW => "w",
        KeyCode::BracketLeft => "[", KeyCode::BracketRight => "]",
        KeyCode::Digit1 => "1", KeyCode::Digit2 => "2", KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4", KeyCode::Digit5 => "5", KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7", KeyCode::Digit8 => "8", KeyCode::Digit9 => "9",
        KeyCode::Digit0 => "0", KeyCode::Equal => "=", KeyCode::Minus => "-",
        _ => return logical.clone(),
    };
    Key::Character(text.into())
}

fn bidi_control(c: char) -> bool {
    matches!(c as u32, 0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
}

/// Preserve UTF-8 and Arabic joiners/marks; normalize all line endings once.
/// Pasted controls must not terminate bracketed paste or become key events.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') { chars.next(); }
                cleaned.push('\r');
            }
            '\n' => cleaned.push('\r'),
            '\t' => cleaned.push(c),
            _ if !c.is_control() && !bidi_control(c) => cleaned.push(c),
            _ => {}
        }
    }
    if bracketed && !cleaned.is_empty() {
        format!("\x1b[200~{cleaned}\x1b[201~").into_bytes()
    } else {
        cleaned.into_bytes()
    }
}

/// Quote a dropped path as one shell argument, never as shell syntax.
pub fn quoted_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text.chars().any(|c| c.is_control() || bidi_control(c)) { return None; }
    Some(format!("'{}' ", text.replace('\'', "'\\''")))
}
