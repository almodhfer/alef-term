//! A discoverable set of terminal actions, with a keyboard-driven filter.
use crate::shortcuts::{Platform, PLATFORM};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab, NextTab, PreviousTab, NewWindow, Search, Theme, ArabicFonts, Reload, Copy, Paste, SelectAll, ClearHistory,
    ZoomIn, ZoomOut, ZoomReset, DirectionAuto, DirectionLtr, DirectionRtl,
    ArabicFont(crate::font::ArabicFont),
}

/// Each action with its label and, for those without keys of their own, a
/// note shown in place of the keys.
pub const ACTIONS: &[(Action, &str, &str)] = &[
    (Action::NewTab, "New tab", ""),
    (Action::NextTab, "Next tab", ""),
    (Action::PreviousTab, "Previous tab", ""),
    (Action::NewWindow, "New window", ""),
    (Action::Search, "Search scrollback", ""),
    (Action::Theme, "Choose color theme", ""),
    (Action::ArabicFonts, "Choose Arabic style", ""),
    (Action::Reload, "Reload settings", ""),
    (Action::Copy, "Copy selection", ""),
    (Action::Paste, "Paste clipboard", ""),
    (Action::SelectAll, "Select all text", ""),
    (Action::ClearHistory, "Clear scrollback", ""),
    (Action::ZoomIn, "Increase font size", ""),
    (Action::ZoomOut, "Decrease font size", ""),
    (Action::ZoomReset, "Reset font size", ""),
    (Action::ArabicFont(crate::font::ArabicFont::Naskh), "Arabic font: Noto Naskh", "Clear Naskh"),
    (Action::ArabicFont(crate::font::ArabicFont::Amiri), "Arabic font: Amiri", "Traditional"),
    (Action::ArabicFont(crate::font::ArabicFont::Scheherazade), "Arabic font: Scheherazade New", "Calligraphic"),
    (Action::ArabicFont(crate::font::ArabicFont::Geeza), "Arabic font: Geeza Pro", "System font"),
    (Action::DirectionAuto, "Text direction: automatic", ""),
    (Action::DirectionLtr, "Text direction: left to right", ""),
    (Action::DirectionRtl, "Text direction: right to left", ""),
];

/// Whether this build offers `action`. Tabs are macOS's native window tabs,
/// and Geeza Pro ships only with macOS.
pub fn offered(action: Action) -> bool {
    PLATFORM == Platform::Mac
        || !matches!(action, Action::NewTab | Action::NextTab | Action::PreviousTab
            | Action::ArabicFont(crate::font::ArabicFont::Geeza))
}

/// Shown at the right of a palette entry: its keys, or else its note.
pub fn hint(id: usize) -> &'static str {
    let (action, _, note) = ACTIONS[id];
    PLATFORM.hint(action).unwrap_or(note)
}

#[derive(Default, Debug)]
pub struct CommandPalette {
    pub query: String,
    pub selected: usize,
}

impl CommandPalette {
    pub fn matches(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        ACTIONS.iter().enumerate().filter(|&(i, &(action, label, _))| {
            let hay = format!("{label} {}", hint(i)).to_lowercase();
            offered(action) && query.split_whitespace().all(|word| hay.contains(word))
        }).map(|(i, _)| i).collect()
    }

    pub fn current(&self) -> Option<Action> {
        self.matches().get(self.selected).map(|&i| ACTIONS[i].0)
    }

    pub fn step(&mut self, delta: isize) {
        let count = self.matches().len();
        self.selected = if count == 0 { 0 } else {
            (self.selected as isize + delta).rem_euclid(count as isize) as usize
        };
    }

    pub fn push(&mut self, text: &str) {
        self.query.extend(text.chars().filter(|c| !c.is_control()).take(256usize.saturating_sub(self.query.chars().count())));
        self.selected = 0;
    }

    pub fn backspace(&mut self, all: bool) {
        if all { self.query.clear(); } else { self.query.pop(); }
        self.selected = 0;
    }
}
