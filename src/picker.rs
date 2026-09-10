//! The theme picker: the built-in themes, filtered by what has been typed,
//! with one of them highlighted. The window previews the highlighted theme
//! live; Enter keeps it and Esc goes back to the one in use before.

use crate::color::{Theme, THEMES};

pub struct ThemePicker {
    pub query: String,
    /// Index into [`ThemePicker::matches`] of the highlighted theme.
    pub selected: usize,
    /// The theme in use when the picker opened, restored on cancel.
    pub original: &'static Theme,
}

impl ThemePicker {
    /// Open on the theme in use.
    pub fn new(current: &'static Theme) -> Self {
        let selected = THEMES.iter().position(|t| std::ptr::eq(t, current)).unwrap_or(0);
        ThemePicker { query: String::new(), selected, original: current }
    }

    /// Themes whose title or name contains the query, ignoring case.
    pub fn matches(&self) -> Vec<&'static Theme> {
        let q = self.query.to_lowercase();
        THEMES
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&q) || t.name.contains(&q))
            .collect()
    }

    /// The highlighted theme, if anything matches.
    pub fn current(&self) -> Option<&'static Theme> {
        self.matches().get(self.selected).copied()
    }

    /// Move the highlight down (`+1`) or up (`-1`), wrapping around.
    pub fn step(&mut self, dir: i32) {
        let n = self.matches().len() as i64;
        if n > 0 {
            self.selected = (self.selected as i64 + dir as i64).rem_euclid(n) as usize;
        }
    }

    pub fn push(&mut self, text: &str) {
        let keep = self.current();
        self.query.push_str(text);
        self.refilter(keep);
    }

    /// Delete the last character, or with `all` the whole query.
    pub fn pop(&mut self, all: bool) {
        let keep = self.current();
        if all {
            self.query.clear();
        } else {
            self.query.pop();
        }
        self.refilter(keep);
    }

    /// Keep the highlight on the same theme if it still matches, else the
    /// first match.
    fn refilter(&mut self, keep: Option<&'static Theme>) {
        let matches = self.matches();
        self.selected = keep
            .and_then(|k| matches.iter().position(|t| std::ptr::eq(*t, k)))
            .unwrap_or(0);
    }
}
