//! Which keys belong to the terminal rather than the program, per platform.
//!
//! On macOS the shortcuts sit on Command, which programs in a terminal never
//! receive. Linux desktops keep Super for themselves, so there the shortcuts
//! use Ctrl+Shift — the convention of GNOME Terminal, Konsole and kitty — and
//! plain Ctrl keys still reach the program, apart from Ctrl +, - and 0 for zoom.
use crate::commands::Action;
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform { Mac, Linux }

/// The conventions this build follows. Anything that is not macOS gets the
/// Linux ones.
pub const PLATFORM: Platform = if cfg!(target_os = "macos") { Platform::Mac } else { Platform::Linux };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Do(Action),
    Palette,
    /// Step the base direction: auto, then LTR, then RTL.
    CycleDirection,
    /// A tab by number, 1 to 9; 9 is always the last tab.
    SelectTab(usize),
    Close,
    Quit,
}

impl Platform {
    /// The shortcut modifier is held: keys pressed with it are the terminal's.
    pub fn command(self, m: ModifiersState) -> bool {
        match self {
            Platform::Mac => m.super_key(),
            Platform::Linux => m.control_key() && m.shift_key() && !m.alt_key() && !m.super_key(),
        }
    }

    /// Worth matching against the shortcut table at all. Wider than
    /// [`Platform::command`] on Linux, where zoom needs only Ctrl.
    pub fn chord(self, m: ModifiersState) -> bool {
        match self {
            Platform::Mac => m.super_key(),
            Platform::Linux => m.control_key(),
        }
    }

    /// Held to underline links; with a click, opens one.
    pub fn link(self, m: ModifiersState) -> bool {
        match self {
            Platform::Mac => m.super_key(),
            Platform::Linux => m.control_key(),
        }
    }

    /// With Backspace, clears a whole input field rather than one character.
    pub fn erase_all(self, m: ModifiersState) -> bool {
        match self {
            Platform::Mac => m.super_key(),
            Platform::Linux => m.control_key(),
        }
    }

    /// The shortcut on `key`, as normalised by [`crate::input::command_key`].
    pub fn shortcut(self, key: &Key, m: ModifiersState) -> Option<Shortcut> {
        match (self, key) {
            (Platform::Mac, Key::Character(c)) => mac(c.as_str(), m),
            (Platform::Linux, Key::Character(c)) => linux(c.as_str(), m),
            (Platform::Linux, Key::Named(named)) => linux_tabs(*named, m),
            _ => None,
        }
    }

    /// `shortcut` has nothing to do here and its key belongs to the program.
    /// On Linux the keys that step between tabs, Ctrl+Tab and Ctrl+PgUp/PgDn,
    /// are ones vim and tmux use too, so a window with one tab leaves them be,
    /// and Ctrl+Shift with a number along with them.
    pub fn passes(self, shortcut: Shortcut, tabbed: bool) -> bool {
        self == Platform::Linux
            && !tabbed
            && matches!(shortcut, Shortcut::Do(Action::NextTab | Action::PreviousTab) | Shortcut::SelectTab(_))
    }

    /// The keys shown beside `action` in the command palette.
    pub fn hint(self, action: Action) -> Option<&'static str> {
        use Action::*;
        Some(match (self, action) {
            (Platform::Mac, NewTab) => "Cmd T",
            (Platform::Mac, NextTab) => "Cmd Shift ]",
            (Platform::Mac, PreviousTab) => "Cmd Shift [",
            (Platform::Mac, NewWindow) => "Cmd N",
            (Platform::Mac, Search) => "Cmd F",
            (Platform::Mac, Theme) => "Cmd Shift T",
            (Platform::Mac, ArabicFonts) => "Cmd Shift F",
            (Platform::Mac, Reload) => "Cmd Shift R",
            (Platform::Mac, Copy) => "Cmd C",
            (Platform::Mac, Paste) => "Cmd V",
            (Platform::Mac, SelectAll) => "Cmd A",
            (Platform::Mac, ClearHistory) => "Cmd K",
            (Platform::Mac, ZoomIn) => "Cmd +",
            (Platform::Mac, ZoomOut) => "Cmd -",
            (Platform::Mac, ZoomReset) => "Cmd 0",
            (Platform::Linux, NewTab) => "Ctrl Shift T",
            (Platform::Linux, NextTab) => "Ctrl Tab",
            (Platform::Linux, PreviousTab) => "Ctrl Shift Tab",
            (Platform::Linux, NewWindow) => "Ctrl Shift N",
            (Platform::Linux, Search) => "Ctrl Shift F",
            (Platform::Linux, Copy) => "Ctrl Shift C",
            (Platform::Linux, Paste) => "Ctrl Shift V",
            (Platform::Linux, SelectAll) => "Ctrl Shift A",
            (Platform::Linux, ClearHistory) => "Ctrl Shift K",
            (Platform::Linux, ZoomIn) => "Ctrl +",
            (Platform::Linux, ZoomOut) => "Ctrl -",
            (Platform::Linux, ZoomReset) => "Ctrl 0",
            _ => return None,
        })
    }
}

fn tab(c: &str) -> Option<usize> {
    c.parse().ok().filter(|n| (1..=9).contains(n))
}

fn mac(c: &str, m: ModifiersState) -> Option<Shortcut> {
    if !m.super_key() { return None; }
    let shift = m.shift_key();
    Some(match c {
        "p" if shift => Shortcut::Palette,
        "r" if shift => Shortcut::Do(Action::Reload),
        "t" if shift => Shortcut::Do(Action::Theme),
        "f" if shift => Shortcut::Do(Action::ArabicFonts),
        "]" | "}" if shift => Shortcut::Do(Action::NextTab),
        "[" | "{" if shift => Shortcut::Do(Action::PreviousTab),
        "t" => Shortcut::Do(Action::NewTab),
        "f" => Shortcut::Do(Action::Search),
        "n" => Shortcut::Do(Action::NewWindow),
        "c" => Shortcut::Do(Action::Copy),
        "v" => Shortcut::Do(Action::Paste),
        "a" => Shortcut::Do(Action::SelectAll),
        "k" => Shortcut::Do(Action::ClearHistory),
        "+" | "=" => Shortcut::Do(Action::ZoomIn),
        "-" => Shortcut::Do(Action::ZoomOut),
        "0" => Shortcut::Do(Action::ZoomReset),
        "r" => Shortcut::CycleDirection,
        "w" => Shortcut::Close,
        "q" => Shortcut::Quit,
        _ => return Some(Shortcut::SelectTab(tab(c)?)),
    })
}

fn linux(c: &str, m: ModifiersState) -> Option<Shortcut> {
    if m == ModifiersState::CONTROL {
        return Some(match c {
            "+" | "=" => Shortcut::Do(Action::ZoomIn),
            "-" => Shortcut::Do(Action::ZoomOut),
            "0" => Shortcut::Do(Action::ZoomReset),
            _ => return None,
        });
    }
    if !Platform::Linux.command(m) { return None; }
    Some(match c {
        "p" => Shortcut::Palette,
        "t" => Shortcut::Do(Action::NewTab),
        "n" => Shortcut::Do(Action::NewWindow),
        "f" => Shortcut::Do(Action::Search),
        "c" => Shortcut::Do(Action::Copy),
        "v" => Shortcut::Do(Action::Paste),
        "a" => Shortcut::Do(Action::SelectAll),
        "k" => Shortcut::Do(Action::ClearHistory),
        "+" | "=" => Shortcut::Do(Action::ZoomIn),
        "-" => Shortcut::Do(Action::ZoomOut),
        "0" => Shortcut::Do(Action::ZoomReset),
        "r" => Shortcut::CycleDirection,
        "w" => Shortcut::Close,
        "q" => Shortcut::Quit,
        _ => return Some(Shortcut::SelectTab(tab(c)?)),
    })
}

/// Stepping through tabs, on the keys browsers and GNOME Terminal use.
fn linux_tabs(key: NamedKey, m: ModifiersState) -> Option<Shortcut> {
    let ctrl = ModifiersState::CONTROL;
    let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
    Some(match key {
        NamedKey::Tab if m == ctrl => Shortcut::Do(Action::NextTab),
        NamedKey::Tab if m == ctrl_shift => Shortcut::Do(Action::PreviousTab),
        NamedKey::PageDown if m == ctrl => Shortcut::Do(Action::NextTab),
        NamedKey::PageUp if m == ctrl => Shortcut::Do(Action::PreviousTab),
        _ => return None,
    })
}
