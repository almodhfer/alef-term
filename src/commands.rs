//! A discoverable set of terminal actions, with a keyboard-driven filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab, NextTab, PreviousTab, NewWindow, Search, Theme, ArabicFonts, Reload, Copy, Paste, SelectAll, ClearHistory,
    ZoomIn, ZoomOut, ZoomReset, DirectionAuto, DirectionLtr, DirectionRtl,
    ArabicFont(crate::font::ArabicFont),
}

pub const ACTIONS: &[(Action, &str, &str)] = &[
    (Action::NewTab, "New tab", "Cmd T"),
    (Action::NextTab, "Next tab", "Cmd Shift ]"),
    (Action::PreviousTab, "Previous tab", "Cmd Shift ["),
    (Action::NewWindow, "New window", "Cmd N"),
    (Action::Search, "Search scrollback", "Cmd F"),
    (Action::Theme, "Choose color theme", "Cmd Shift T"),
    (Action::ArabicFonts, "Choose Arabic style", "Cmd Shift F"),
    (Action::Reload, "Reload settings", "Cmd Shift R"),
    (Action::Copy, "Copy selection", "Cmd C"),
    (Action::Paste, "Paste clipboard", "Cmd V"),
    (Action::SelectAll, "Select all text", "Cmd A"),
    (Action::ClearHistory, "Clear scrollback", "Cmd K"),
    (Action::ZoomIn, "Increase font size", "Cmd +"),
    (Action::ZoomOut, "Decrease font size", "Cmd -"),
    (Action::ZoomReset, "Reset font size", "Cmd 0"),
    (Action::ArabicFont(crate::font::ArabicFont::Naskh), "Arabic font: Noto Naskh", "Clear Naskh"),
    (Action::ArabicFont(crate::font::ArabicFont::Amiri), "Arabic font: Amiri", "Traditional"),
    (Action::ArabicFont(crate::font::ArabicFont::Scheherazade), "Arabic font: Scheherazade New", "Calligraphic"),
    (Action::ArabicFont(crate::font::ArabicFont::Geeza), "Arabic font: Geeza Pro", "System font"),
    (Action::DirectionAuto, "Text direction: automatic", ""),
    (Action::DirectionLtr, "Text direction: left to right", ""),
    (Action::DirectionRtl, "Text direction: right to left", ""),
];

#[derive(Default, Debug)]
pub struct CommandPalette {
    pub query: String,
    pub selected: usize,
}

impl CommandPalette {
    pub fn matches(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        ACTIONS.iter().enumerate().filter(|(_, (_, label, hint))| {
            let hay = format!("{label} {hint}").to_lowercase();
            query.split_whitespace().all(|word| hay.contains(word))
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
