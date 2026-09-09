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
        // Note: the viewport is *not* adjusted here. While scrolled back the
        // app owns `view_offset`, driven from its `Scroller`, and it follows
        // retiring lines using `Grid::scrolled`. Bumping it here as well would
        // double-count and make the view drift.
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

    /// Index, in the combined scrollback+screen sequence, of the first line
    /// shown in the viewport.
    #[inline]
    pub fn viewport_top(&self) -> isize {
        self.scrollback.len() as isize - self.view_offset as isize
    }

    /// The row at an absolute index across scrollback then screen, if it
    /// exists. Indices outside the range return `None`, which is what lets the
    /// renderer ask for the partially-visible line above the viewport while
    /// smooth-scrolling.
    pub fn line(&self, abs: isize) -> Option<&Row> {
        if abs < 0 {
            return None;
        }
        let abs = abs as usize;
        let sb = self.scrollback.len();
        if abs < sb {
            self.scrollback.get(abs)
        } else {
            self.screen.rows.get(abs - sb)
        }
    }

    /// The row displayed at viewport line `i`, honouring scrollback offset.
    pub fn viewport_row(&self, i: usize) -> &Row {
        self.line(self.viewport_top() + i as isize).unwrap_or(&self.screen.rows[0])
    }

    /// Total lines available to scroll through.
    #[inline]
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Change the grid size.
    ///
    /// When the width changes, the primary screen and its scrollback are
    /// re-wrapped: rows that autowrap broke are joined back into the lines
    /// the program actually wrote and broken again at the new width, so a
    /// narrower window shows more rows of the same text rather than cutting
    /// it off, and widening puts it back together. Explicit newlines are
    /// kept. The alternate screen is never reflowed — a full-screen program
    /// repaints itself — so while one is showing, its buffer is just clamped
    /// and the primary underneath is reflowed for when it returns.
    ///
    /// Returns how absolute line numbers moved, when they did, so that
    /// anything pinned to a line (images) can follow.
    pub fn resize(&mut self, rows: usize, cols: usize) -> Option<ReflowMap> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let mut map = None;
        if cols != self.cols {
            if self.in_alt() {
                let primary = self.alt.take().expect("in alt");
                let (primary, m) = self.reflow(primary, rows, cols);
                self.alt = Some(primary);
                map = Some(m);
                Self::clamp_screen(&mut self.screen, rows, cols);
            } else {
                let screen = std::mem::replace(&mut self.screen, Screen::new(1, 1));
                let (screen, m) = self.reflow(screen, rows, cols);
                self.screen = screen;
                map = Some(m);
            }
        } else {
            self.resize_rows(rows);
        }
        if let Some(alt) = &mut self.alt {
            Self::clamp_screen(alt, rows, cols);
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
        map
    }

    /// Height change only: rows keep their text, and the prompt stays put by
    /// pulling lines back from scrollback when growing and retiring them when
    /// shrinking.
    fn resize_rows(&mut self, rows: usize) {
        let cols = self.cols;
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
    }

    /// Truncate or pad a screen without reflowing it.
    fn clamp_screen(s: &mut Screen, rows: usize, cols: usize) {
        for r in &mut s.rows {
            r.resize(cols);
        }
        s.rows.resize_with(rows, || Row::new(cols));
        s.scroll_top = 0;
        s.scroll_bot = rows;
        s.cursor.row = s.cursor.row.min(rows - 1);
        s.cursor.col = s.cursor.col.min(cols - 1);
        s.saved_cursor.row = s.saved_cursor.row.min(rows - 1);
        s.saved_cursor.col = s.saved_cursor.col.min(cols - 1);
    }

    /// Re-wrap `screen` and the scrollback at `cols` columns into a screen of
    /// `rows` rows, moving cells rather than copying them.
    fn reflow(&mut self, screen: Screen, rows: usize, cols: usize) -> (Screen, ReflowMap) {
        let old_rows = screen.rows.len();
        let old_sb = self.scrollback.len();
        let base = self.scrolled as i64 - old_sb as i64;

        // Blank rows below the content and the cursor are padding, not text:
        // they must not turn into lines that push real ones into scrollback.
        let content_bottom = screen
            .rows
            .iter()
            .rposition(|r| r.wrapped || r.trimmed_len() > 0)
            .unwrap_or(0)
            .max(screen.cursor.row.min(old_rows - 1));
        let padding = old_rows - 1 - content_bottom;
        let mut screen_rows = screen.rows;
        screen_rows.truncate(content_bottom + 1);

        // 1. Join autowrapped rows back into logical lines. Each old row is
        //    remembered as (line, cell offset) so cursors and pins can follow.
        let mut lines: Vec<Vec<Cell>> = Vec::with_capacity(old_sb + old_rows);
        let mut where_old: Vec<(usize, usize)> = Vec::with_capacity(old_sb + old_rows);
        let mut cur: Vec<Cell> = Vec::new();
        let old = self.scrollback.drain(..).chain(screen_rows.into_iter());
        for mut row in old {
            // A blank left where a wide character did not fit at the margin
            // is filler, not text: drop it when the character follows.
            let starts_wide = row.cells.first().is_some_and(|c| c.attrs.contains(Attrs::WIDE_LEADING));
            if starts_wide && cur.last().is_some_and(|c| c.is_blank() && c.attrs.is_empty()) {
                cur.pop();
            }
            where_old.push((lines.len(), cur.len()));
            if row.wrapped {
                cur.append(&mut row.cells);
            } else {
                let keep = row.trimmed_len();
                row.cells.truncate(keep);
                cur.append(&mut row.cells);
                lines.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        // The padding rows map to just past the text.
        for i in 0..padding {
            where_old.push((lines.len(), i * cols));
        }

        // 2. Break each line at the new width. A wide character is never split
        //    across rows: it moves whole to the next one, leaving a blank.
        let mut new_rows: Vec<Row> = Vec::with_capacity(lines.len());
        let mut line_start: Vec<usize> = Vec::with_capacity(lines.len());
        for line in lines {
            line_start.push(new_rows.len());
            let mut it = line.into_iter().peekable();
            let mut first = true;
            while first || it.peek().is_some() {
                first = false;
                let mut cells: Vec<Cell> = Vec::with_capacity(cols);
                while cells.len() < cols {
                    match it.peek() {
                        None => break,
                        Some(c) if cells.len() == cols - 1 && c.attrs.contains(Attrs::WIDE_LEADING) && cols > 1 => break,
                        Some(_) => cells.push(it.next().unwrap()),
                    }
                }
                let wrapped = it.peek().is_some();
                cells.resize(cols, Cell::default());
                new_rows.push(Row { cells, wrapped });
            }
        }
        let total = new_rows.len();
        line_start.push(total);

        // 3. Where a cursor landed, in new-row terms.
        let locate = |c: &Cursor| -> (usize, usize, bool) {
            let old_idx = (old_sb + c.row.min(old_rows - 1)).min(where_old.len() - 1);
            let (line, off) = where_old[old_idx];
            let off = off + c.col;
            let start = line_start[line];
            let end = line_start.get(line + 1).copied().unwrap_or(total);
            if line >= line_start.len() - 1 {
                // A cursor on a padding row: past every line.
                return (total.saturating_sub(1) + (off / cols) + 1, off % cols, false);
            }
            let chunks = (end - start).max(1);
            let mut r = off / cols;
            let mut col = off % cols;
            let mut pending = false;
            if r >= chunks {
                // Past the text: the last row, deferring the wrap if it was
                // already deferred.
                r = chunks - 1;
                col = (off - r * cols).min(cols - 1);
                pending = c.pending_wrap && off == chunks * cols;
            }
            (start + r, col, pending)
        };
        let (cur_idx, cur_col, cur_pending) = locate(&screen.cursor);
        let (sav_idx, sav_col, _) = locate(&screen.saved_cursor);

        // 4. Split into scrollback and screen. Text that fits stays anchored
        //    at the top, so narrowing wraps into the blank space below rather
        //    than pushing lines off the top; once it overflows, the screen
        //    shows the end, and always the cursor.
        let mut first_on_screen = total.saturating_sub(rows);
        if cur_idx < first_on_screen {
            first_on_screen = cur_idx;
        }
        let visible = (total - first_on_screen).min(rows);
        let mut rest = new_rows.split_off(first_on_screen);
        rest.truncate(visible);
        rest.resize_with(rows, || Row::new(cols));
        self.scrollback = new_rows.into();
        let dropped = self.scrollback.len().saturating_sub(self.max_scrollback);
        for _ in 0..dropped {
            self.scrollback.pop_front();
        }
        self.scrolled = (base + first_on_screen as i64).max(0) as u64;

        let mut new_screen = Screen::new(rows, cols);
        new_screen.rows = rest;
        new_screen.cursor = Cursor {
            row: cur_idx.saturating_sub(first_on_screen).min(rows - 1),
            col: cur_col,
            pending_wrap: cur_pending,
            ..screen.cursor
        };
        new_screen.saved_cursor = Cursor {
            row: sav_idx.saturating_sub(first_on_screen).min(rows - 1),
            col: sav_col,
            pending_wrap: false,
            ..screen.saved_cursor
        };

        let new_row_of_old = where_old
            .iter()
            .map(|&(line, off)| line_start[line] + off / cols)
            .collect();
        (new_screen, ReflowMap { base, new_row_of_old })
    }
}

/// How a reflow moved lines: old absolute line `base + i` became
/// `base + new_row_of_old[i]`.
pub struct ReflowMap {
    base: i64,
    new_row_of_old: Vec<usize>,
}

impl ReflowMap {
    /// The new absolute line of an old one, if it still exists.
    pub fn map(&self, old_abs: u64) -> Option<u64> {
        let i = old_abs as i64 - self.base;
        if i < 0 {
            return None;
        }
        let new = *self.new_row_of_old.get(i as usize)?;
        Some((self.base + new as i64).max(0) as u64)
    }
}
