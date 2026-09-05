//! The terminal screen model.
//!
//! The grid stores text in **logical order** — exactly the sequence the client
//! program wrote. Bidi reordering and Arabic shaping happen only at render
//! time (see `layout`). Keeping storage logical is what lets column arithmetic
//! in `vim`, `htop` and friends keep working when a row contains Arabic.

use crate::color::Color;
use std::collections::VecDeque;

bitflags::bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub struct Attrs: u16 {
        const BOLD          = 1 << 0;
        const DIM           = 1 << 1;
        const ITALIC        = 1 << 2;
        const UNDERLINE     = 1 << 3;
        const BLINK         = 1 << 4;
        const INVERSE       = 1 << 5;
        const HIDDEN        = 1 << 6;
        const STRIKEOUT     = 1 << 7;
        /// Second half of a double-width character; carries no glyph.
        const WIDE_SPACER   = 1 << 8;
        /// Leading half of a double-width character.
        const WIDE_LEADING  = 1 << 9;
    }
}

/// Rarely-populated per-cell data, boxed so the common cell stays small.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CellExtra {
    /// Combining marks attached to this cell's base character. Arabic stacks
    /// these freely (shadda + fatha, plus Quranic annotation marks), so this is
    /// a growable list rather than a fixed slot.
    pub marks: Vec<char>,
    /// OSC 8 hyperlink target.
    pub link: Option<Box<str>>,
}

/// One grid cell. Kept at 24 bytes so a 10k-line scrollback at 200 columns
/// costs ~48 MB rather than hundreds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub extra: Option<Box<CellExtra>>,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
            extra: None,
        }
    }
}

impl Cell {
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.extra.is_none() && self.bg == Color::Default
    }

    /// Attach a zero-width combining mark to this cell.
    pub fn push_mark(&mut self, c: char) {
        let extra = self.extra.get_or_insert_with(Default::default);
        // Guard against unbounded growth from a hostile stream.
        if extra.marks.len() < 8 {
            extra.marks.push(c);
        }
    }

    pub fn marks(&self) -> &[char] {
        self.extra.as_deref().map_or(&[], |e| e.marks.as_slice())
    }

    /// Reset to blank while inheriting the given background (ECMA-48 says
    /// erasures take the current background colour).
    pub fn reset(&mut self, bg: Color) {
        self.ch = ' ';
        self.fg = Color::Default;
        self.bg = bg;
        self.attrs = Attrs::empty();
        self.extra = None;
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Set when the line was broken by autowrap rather than an explicit
    /// newline; needed for reflow and for copy that rejoins wrapped lines.
    pub wrapped: bool,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Row { cells: vec![Cell::default(); cols], wrapped: false }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
    }

    pub fn reset(&mut self, bg: Color) {
        for c in &mut self.cells {
            c.reset(bg);
        }
        self.wrapped = false;
    }

    /// Index of the last non-blank cell, for trimming trailing whitespace on
    /// copy and for cheap dirty comparisons.
    pub fn trimmed_len(&self) -> usize {
        self.cells.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    /// Deferred autowrap: the cursor sits on the last column and the *next*
    /// printable character wraps. Tracking this separately from `col` is what
    /// makes writing to the bottom-right cell behave like a real terminal.
    pub pending_wrap: bool,
}

/// One screen buffer (primary or alternate).
#[derive(Clone)]
pub struct Screen {
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    pub saved_cursor: Cursor,
    /// Inclusive top / exclusive bottom of the DECSTBM scrolling region.
    pub scroll_top: usize,
    pub scroll_bot: usize,
}

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Self {
        Screen {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            cursor: Cursor::default(),
            saved_cursor: Cursor::default(),
            scroll_top: 0,
            scroll_bot: rows,
        }
    }
}

pub struct Grid {
    pub screen: Screen,
    pub alt: Option<Screen>,
    pub scrollback: VecDeque<Row>,
    pub max_scrollback: usize,
    pub rows: usize,
    pub cols: usize,
    /// How far the viewport is scrolled back, in lines above the live screen.
    pub view_offset: usize,
    /// Total lines ever scrolled off the top. Screen row `r` is absolute line
    /// `scrolled + r`, which is how images stay pinned to their text.
    pub scrolled: u64,
}

impl Grid {
    pub fn new(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Grid {
            screen: Screen::new(rows, cols),
            alt: None,
            scrollback: VecDeque::new(),
            max_scrollback,
            rows,
            cols,
            view_offset: 0,
            scrolled: 0,
        }
    }

    #[inline]
    pub fn in_alt(&self) -> bool {
        self.alt.is_some()
    }

    /// Scroll the region up by `n` lines. Lines leaving the top of a full-screen
    /// region on the primary screen are pushed to scrollback; lines leaving a
    /// restricted region, or any line on the alternate screen, are discarded.
    pub fn scroll_up(&mut self, n: usize, bg: Color) {
        let (top, bot) = (self.screen.scroll_top, self.screen.scroll_bot);
        let n = n.min(bot.saturating_sub(top));
        if n == 0 {
            return;
        }
        let to_scrollback = !self.in_alt() && top == 0 && bot == self.rows;
        if to_scrollback {
            self.scrolled += n as u64;
        }
        for _ in 0..n {
            let mut row = self.screen.rows.remove(top);
            if to_scrollback && self.max_scrollback > 0 {
                self.scrollback.push_back(std::mem::replace(&mut row, Row::new(self.cols)));
                while self.scrollback.len() > self.max_scrollback {
                    self.scrollback.pop_front();
                }
            }
            row.resize(self.cols);
            row.reset(bg);
            self.screen.rows.insert(bot - 1, row);
        }
        // Keep the viewport anchored to the same content while scrolled back.
        if self.view_offset > 0 {
            self.view_offset = (self.view_offset + n).min(self.scrollback.len());
        }
    }

    /// Scroll the region down by `n` lines (content moves toward the bottom).
    pub fn scroll_down(&mut self, n: usize, bg: Color) {
        let (top, bot) = (self.screen.scroll_top, self.screen.scroll_bot);
        let n = n.min(bot.saturating_sub(top));
        for _ in 0..n {
            let mut row = self.screen.rows.remove(bot - 1);
            row.resize(self.cols);
            row.reset(bg);
            self.screen.rows.insert(top, row);
        }
    }

    /// The row displayed at viewport line `i`, honouring scrollback offset.
    pub fn viewport_row(&self, i: usize) -> &Row {
        if self.view_offset == 0 {
            return &self.screen.rows[i];
        }
        let sb = self.scrollback.len();
        let start = sb.saturating_sub(self.view_offset);
        match self.scrollback.get(start + i) {
            Some(r) => r,
            None => &self.screen.rows[start + i - sb],
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        for r in &mut self.screen.rows {
            r.resize(cols);
        }
        if let Some(alt) = &mut self.alt {
            for r in &mut alt.rows {
                r.resize(cols);
            }
        }

        // Growing: pull lines back off scrollback so the prompt stays put.
        while self.screen.rows.len() < rows {
            match (self.scrollback.pop_back(), self.alt.is_some()) {
                (Some(mut row), false) => {
                    row.resize(cols);
                    self.screen.rows.insert(0, row);
                    self.screen.cursor.row += 1;
                }
                _ => self.screen.rows.push(Row::new(cols)),
            }
        }
        // Shrinking: retire lines from the top into scrollback.
        while self.screen.rows.len() > rows {
            let from_top = self.screen.cursor.row + 1 >= rows;
            if from_top {
                let row = self.screen.rows.remove(0);
                if !self.in_alt() && self.max_scrollback > 0 {
                    self.scrollback.push_back(row);
                }
                self.screen.cursor.row = self.screen.cursor.row.saturating_sub(1);
            } else {
                self.screen.rows.pop();
            }
        }
        if let Some(alt) = &mut self.alt {
            alt.rows.resize_with(rows, || Row::new(cols));
            alt.scroll_bot = alt.scroll_bot.min(rows).max(1);
            alt.cursor.row = alt.cursor.row.min(rows - 1);
            alt.cursor.col = alt.cursor.col.min(cols - 1);
        }

        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }

        self.rows = rows;
        self.cols = cols;
        self.screen.scroll_top = 0;
        self.screen.scroll_bot = rows;
        self.screen.cursor.row = self.screen.cursor.row.min(rows - 1);
        self.screen.cursor.col = self.screen.cursor.col.min(cols - 1);
        self.view_offset = self.view_offset.min(self.scrollback.len());
    }
}
