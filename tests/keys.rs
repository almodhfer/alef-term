//! Key encoding: what actually goes down the pipe when you press a key.

use arabic_term::keys::{encode, KeyContext};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

fn ch(s: &str) -> Key {
    Key::Character(SmolStr::new(s))
}
fn plain(key: &Key, text: Option<&str>) -> Vec<u8> {
    encode(key, text, ModifiersState::empty(), KeyContext::default()).expect("encoded")
}
fn with(key: &Key, text: Option<&str>, mods: ModifiersState) -> Vec<u8> {
    encode(key, text, mods, KeyContext::default()).expect("encoded")
}

#[test]
fn plain_ascii_goes_through_as_typed() {
    assert_eq!(plain(&ch("a"), Some("a")), b"a");
    assert_eq!(plain(&Key::Named(NamedKey::Enter), None), b"\r");
    assert_eq!(plain(&Key::Named(NamedKey::Tab), None), b"\t");
    assert_eq!(plain(&Key::Named(NamedKey::Escape), None), b"\x1b");
    // Backspace sends DEL, which is what stty and readline expect.
    assert_eq!(plain(&Key::Named(NamedKey::Backspace), None), b"\x7f");
}

#[test]
fn arabic_typing_needs_no_special_case() {
    // Whatever the OS layout produced is sent as UTF-8. This is the whole of
    // the Arabic input path.
    assert_eq!(plain(&ch("م"), Some("م")), "م".as_bytes());
    assert_eq!(plain(&ch("ا"), Some("ا")), "ا".as_bytes());
    // A diacritic typed on its own is still just text.
    assert_eq!(plain(&ch("\u{0651}"), Some("\u{0651}")), "\u{0651}".as_bytes());
}

#[test]
fn ctrl_letters_become_control_codes() {
    let ctrl = ModifiersState::CONTROL;
    assert_eq!(with(&ch("c"), Some("c"), ctrl), b"\x03", "Ctrl+C is ETX");
    assert_eq!(with(&ch("d"), Some("d"), ctrl), b"\x04", "Ctrl+D is EOT");
    assert_eq!(with(&ch("z"), Some("z"), ctrl), b"\x1a");
    assert_eq!(with(&ch("a"), Some("a"), ctrl), b"\x01");
    assert_eq!(with(&ch("["), Some("["), ctrl), b"\x1b");
}

#[test]
fn alt_prefixes_escape_as_meta() {
    let alt = ModifiersState::ALT;
    assert_eq!(with(&ch("b"), Some("b"), alt), b"\x1bb", "Alt+B is meta-b");
    assert_eq!(with(&ch("f"), Some("f"), alt), b"\x1bf");
}

#[test]
fn arrows_follow_the_cursor_key_mode() {
    let normal = KeyContext { app_cursor: false, ..Default::default() };
    let app = KeyContext { app_cursor: true, ..Default::default() };
    let up = Key::Named(NamedKey::ArrowUp);
    let e = ModifiersState::empty();
    assert_eq!(encode(&up, None, e, normal).unwrap(), b"\x1b[A");
    assert_eq!(encode(&up, None, e, app).unwrap(), b"\x1bOA", "DECCKM sends SS3");
    assert_eq!(
        encode(&Key::Named(NamedKey::ArrowLeft), None, e, normal).unwrap(),
        b"\x1b[D"
    );
}

#[test]
fn modified_arrows_carry_an_xterm_modifier_parameter() {
    let right = Key::Named(NamedKey::ArrowRight);
    let k = KeyContext::default();
    // shift=2, alt=3, ctrl=5 per xterm's 1 + shift + 2*alt + 4*ctrl.
    assert_eq!(encode(&right, None, ModifiersState::SHIFT, k).unwrap(), b"\x1b[1;2C");
    assert_eq!(encode(&right, None, ModifiersState::ALT, k).unwrap(), b"\x1b[1;3C");
    assert_eq!(encode(&right, None, ModifiersState::CONTROL, k).unwrap(), b"\x1b[1;5C");
}

#[test]
fn navigation_and_function_keys() {
    let e = ModifiersState::empty();
    let k = KeyContext::default();
    let enc = |n: NamedKey| encode(&Key::Named(n), None, e, k).unwrap();
    assert_eq!(enc(NamedKey::Delete), b"\x1b[3~");
    assert_eq!(enc(NamedKey::PageUp), b"\x1b[5~");
    assert_eq!(enc(NamedKey::PageDown), b"\x1b[6~");
    assert_eq!(enc(NamedKey::Home), b"\x1b[H");
    assert_eq!(enc(NamedKey::End), b"\x1b[F");
    assert_eq!(enc(NamedKey::F1), b"\x1bOP");
    assert_eq!(enc(NamedKey::F5), b"\x1b[15~");
    assert_eq!(enc(NamedKey::F12), b"\x1b[24~");
    // Shift+Tab is back-tab.
    assert_eq!(
        encode(&Key::Named(NamedKey::Tab), None, ModifiersState::SHIFT, k).unwrap(),
        b"\x1b[Z"
    );
}

#[test]
fn f1_to_f4_use_ss3_regardless_of_cursor_key_mode() {
    // xterm introduces F1..F4 with SS3 whether or not DECCKM is set; only the
    // arrows and Home/End switch. Getting this wrong misfires F-keys in htop.
    let e = ModifiersState::empty();
    for app_cursor in [false, true] {
        let k = KeyContext { app_cursor, ..Default::default() };
        assert_eq!(encode(&Key::Named(NamedKey::F1), None, e, k).unwrap(), b"\x1bOP");
        assert_eq!(encode(&Key::Named(NamedKey::F4), None, e, k).unwrap(), b"\x1bOS");
    }
}

fn kitty(flags: u8) -> KeyContext {
    KeyContext { app_cursor: false, kitty_flags: flags }
}

#[test]
fn kitty_disambiguates_escape_from_alt() {
    // The whole point of the mode: Esc alone must be distinguishable from the
    // ESC prefix that Alt+key produces.
    let e = ModifiersState::empty();
    assert_eq!(encode(&Key::Named(NamedKey::Escape), None, e, kitty(1)).unwrap(), b"\x1b[27u");
    // Off, it stays the bare byte.
    assert_eq!(
        encode(&Key::Named(NamedKey::Escape), None, e, kitty(0)).unwrap(),
        b"\x1b"
    );
}

#[test]
fn kitty_reports_ctrl_letters_as_csi_u() {
    // Legacy Ctrl+I and Tab are both 0x09; under kitty they differ.
    let ctrl = ModifiersState::CONTROL;
    assert_eq!(encode(&ch("i"), Some("i"), ctrl, kitty(1)).unwrap(), b"\x1b[105;5u");
    assert_eq!(encode(&Key::Named(NamedKey::Tab), None, ModifiersState::empty(), kitty(1)).unwrap(), b"\t");
    // And without the mode, Ctrl+I collapses to Tab's byte as before.
    assert_eq!(encode(&ch("i"), Some("i"), ctrl, kitty(0)).unwrap(), b"\x09");
}

#[test]
fn kitty_modifier_bitmask_matches_the_spec() {
    // 1 + shift(1) + alt(2) + ctrl(4).
    let both = ModifiersState::CONTROL | ModifiersState::SHIFT;
    assert_eq!(encode(&ch("a"), Some("a"), both, kitty(1)).unwrap(), b"\x1b[97;6u");
}

#[test]
fn kitty_leaves_ordinary_typing_alone() {
    // Plain text must still be plain text, or Arabic input would break.
    let e = ModifiersState::empty();
    assert_eq!(encode(&ch("a"), Some("a"), e, kitty(1)).unwrap(), b"a");
    assert_eq!(encode(&ch("م"), Some("م"), e, kitty(1)).unwrap(), "م".as_bytes());
}
