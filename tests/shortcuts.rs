//! Terminal shortcuts on both platforms: the macOS table stays on Command, and
//! Linux leaves plain Ctrl keys to the program.
use arabic_term::commands::{Action, ACTIONS};
use arabic_term::shortcuts::{Platform, Shortcut};
use winit::keyboard::{Key, ModifiersState};

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
            assert_eq!(platform.shortcut(&key(&last.to_lowercase()), mods), Some(Shortcut::Do(action)),
                "{platform:?}: {hint}");
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
