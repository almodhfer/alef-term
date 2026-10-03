//! Terminal shortcuts on both platforms: the macOS table stays on Command, and
//! Linux leaves plain Ctrl keys to the program.
use arabic_term::commands::{Action, ACTIONS};
use arabic_term::shortcuts::{Platform, Shortcut};
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn key(s: &str) -> Key {
    Key::Character(s.into())
}

const CMD: ModifiersState = ModifiersState::SUPER;
const CTRL: ModifiersState = ModifiersState::CONTROL;
const SHIFT: ModifiersState = ModifiersState::SHIFT;
const ALT: ModifiersState = ModifiersState::ALT;

#[test]
fn palette_hints_name_keys_that_perform_their_action() {
    for platform in [Platform::Mac, Platform::Linux] {
        for &(action, _, _) in ACTIONS {
            let Some(hint) = platform.hint(action) else { continue };
            let mut mods = ModifiersState::empty();
            let mut last = "";
            for word in hint.split(' ') {
                match word {
                    "Cmd" => mods |= CMD,
                    "Ctrl" => mods |= CTRL,
                    "Shift" => mods |= SHIFT,
                    k => last = k,
                }
            }
            let k = if last == "Tab" { Key::Named(NamedKey::Tab) } else { key(&last.to_lowercase()) };
            assert_eq!(platform.shortcut(&k, mods), Some(Shortcut::Do(action)), "{platform:?}: {hint}");
        }
    }
}

#[test]
fn mac_shortcuts_stay_on_command() {
    let mac = Platform::Mac;
    assert_eq!(mac.shortcut(&key("c"), CMD), Some(Shortcut::Do(Action::Copy)));
    assert_eq!(mac.shortcut(&key("p"), CMD | SHIFT), Some(Shortcut::Palette));
    assert_eq!(mac.shortcut(&key("p"), CMD), None);
    assert_eq!(mac.shortcut(&key("r"), CMD), Some(Shortcut::CycleDirection));
    assert_eq!(mac.shortcut(&key("r"), CMD | SHIFT), Some(Shortcut::Do(Action::Reload)));
    assert_eq!(mac.shortcut(&key("}"), CMD | SHIFT), Some(Shortcut::Do(Action::NextTab)));
    assert_eq!(mac.shortcut(&key("9"), CMD), Some(Shortcut::SelectTab(9)));
    assert_eq!(mac.shortcut(&key("c"), CTRL), None, "Ctrl C is the program's interrupt");
}

#[test]
fn linux_shortcuts_use_ctrl_shift_and_leave_ctrl_to_programs() {
    let linux = Platform::Linux;
    assert_eq!(linux.shortcut(&key("c"), CTRL | SHIFT), Some(Shortcut::Do(Action::Copy)));
    assert_eq!(linux.shortcut(&key("v"), CTRL | SHIFT), Some(Shortcut::Do(Action::Paste)));
    assert_eq!(linux.shortcut(&key("p"), CTRL | SHIFT), Some(Shortcut::Palette));
    assert_eq!(linux.shortcut(&key("r"), CTRL | SHIFT), Some(Shortcut::CycleDirection));
    assert_eq!(linux.shortcut(&key("w"), CTRL | SHIFT), Some(Shortcut::Close));
    for c in ["a", "c", "f", "k", "n", "p", "q", "r", "t", "v", "w"] {
        assert_eq!(linux.shortcut(&key(c), CTRL), None, "Ctrl {c} belongs to the program");
    }
    assert_eq!(linux.shortcut(&key("="), CTRL), Some(Shortcut::Do(Action::ZoomIn)));
    assert_eq!(linux.shortcut(&key("c"), CTRL | SHIFT | ALT), None);
    assert_eq!(linux.shortcut(&key("c"), CMD), None, "Super belongs to the desktop");
    assert!(linux.command(CTRL | SHIFT) && !linux.command(CTRL));
    assert!(linux.link(CTRL) && !linux.link(CMD));
}

#[test]
fn linux_leaves_the_tab_stepping_keys_to_the_program_in_a_window_with_one_tab() {
    let linux = Platform::Linux;
    for step in [Shortcut::Do(Action::NextTab), Shortcut::Do(Action::PreviousTab), Shortcut::SelectTab(2), Shortcut::SelectTab(9)] {
        assert!(linux.passes(step, false), "{step:?} has no tab to go to");
        assert!(!linux.passes(step, true), "{step:?} moves between tabs");
        assert!(!Platform::Mac.passes(step, false), "Command chords never reach the program");
    }
    for kept in [Shortcut::Do(Action::NewTab), Shortcut::Do(Action::Copy), Shortcut::Do(Action::ZoomReset), Shortcut::Close] {
        assert!(!linux.passes(kept, false), "{kept:?} is still the terminal's");
    }
}

#[test]
fn linux_tabs_step_with_ctrl_tab_and_page_keys_and_jump_by_number() {
    let linux = Platform::Linux;
    let named = |k| Key::Named(k);
    assert_eq!(linux.shortcut(&named(NamedKey::Tab), CTRL), Some(Shortcut::Do(Action::NextTab)));
    assert_eq!(linux.shortcut(&named(NamedKey::Tab), CTRL | SHIFT), Some(Shortcut::Do(Action::PreviousTab)));
    assert_eq!(linux.shortcut(&named(NamedKey::PageDown), CTRL), Some(Shortcut::Do(Action::NextTab)));
    assert_eq!(linux.shortcut(&named(NamedKey::PageUp), CTRL), Some(Shortcut::Do(Action::PreviousTab)));
    assert_eq!(linux.shortcut(&key("3"), CTRL | SHIFT), Some(Shortcut::SelectTab(3)));
    assert_eq!(linux.shortcut(&named(NamedKey::Tab), ModifiersState::empty()), None, "a plain Tab is the program's");
    assert_eq!(linux.shortcut(&key("3"), CTRL), None);
    assert_eq!(Platform::Mac.shortcut(&named(NamedKey::Tab), CTRL), None, "macOS keeps its own tab keys");
}
