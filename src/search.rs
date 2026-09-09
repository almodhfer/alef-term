//! Scrollback search.
//!
//! The grid is flattened into one UTF-8 string, wrapped rows joined so a word
//! broken by the margin is still found, and searched with a SIMD substring
//! finder. Building the index costs a few milliseconds for ten thousand
//! lines and is only redone when the content changed; a query over it is
//! well under a millisecond. Matches come back as cell spans, which is what
//! the renderer highlights and the view scrolls to.

use crate::grid::{Attrs, Grid};
use crate::selection::Span;
use memchr::memmem;

/// Where a row's text starts in the flattened string.
#[derive(Clone, Copy)]
struct RowRef {
    /// Absolute line, in the scrollback-then-screen index space.
    line: usize,
    /// Byte offset of the row's first character.
    start: usize,
    /// Number of cells of text the row contributed (trailing blanks trimmed
    /// unless the row wraps).
    len: usize,
}

#[derive(Default)]
pub struct Index {
    text: String,
    /// `text` with ASCII letters lowered, for case-insensitive queries. The
    /// byte offsets are identical, which is the point.
    lower: String,
    rows: Vec<RowRef>,
    /// What the index was built from; see [`Index::update`].
    key: Option<(u64, usize, usize, usize, u64)>,
}

/// One hit: `count` consecutive entries in the span list, starting at
/// `first`. A hit that crosses a soft wrap has one span per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub first: usize,
    pub count: usize,
    /// The first line, for ordering and scrolling.
    pub line: usize,
}

/// Cap on hits, so a query like `e` over a huge history stays cheap.
pub const MAX_MATCHES: usize = 20_000;

impl Index {
    /// Rebuild if the grid changed since the last build. `epoch` is the
    /// caller's count of content changes; the rest is read from the grid.
    pub fn update(&mut self, grid: &Grid, epoch: u64) -> bool {
        let key = (grid.scrolled, grid.scrollback.len(), grid.rows, grid.cols, epoch);
        if self.key == Some(key) {
            return false;
        }
        self.build(grid);
        self.key = Some(key);
        true
    }

    /// Flatten the grid.
    pub fn build(&mut self, grid: &Grid) {
        self.text.clear();
        self.rows.clear();
        let all = grid.scrollback.iter().chain(grid.screen.rows.iter());
        for (line, row) in all.enumerate() {
            let len = if row.wrapped { row.cells.len() } else { row.trimmed_len() };
            self.rows.push(RowRef { line, start: self.text.len(), len });
            for c in &row.cells[..len] {
                if !c.attrs.contains(Attrs::WIDE_SPACER) {
                    self.text.push(c.ch);
                }
            }
            if !row.wrapped {
                self.text.push('\n');
            }
        }
        self.lower.clear();
        self.lower.push_str(&self.text);
        self.lower.make_ascii_lowercase();
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every hit for `query`, in line order, as (hits, spans).
    ///
    /// A query with no upper-case ASCII letter is matched without regard to
    /// case; one with any is matched exactly. Arabic vowel marks in the query
    /// are dropped, since the index holds base letters only, so a search
    /// typed without harakat finds text written with them.
    pub fn find(&self, query: &str) -> (Vec<Match>, Vec<Span>) {
        let needle: String = query.chars().filter(|c| !is_arabic_mark(*c)).collect();
        if needle.is_empty() || self.rows.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let (hay, needle) = if needle.bytes().any(|b| b.is_ascii_uppercase()) {
            (self.text.as_str(), needle)
        } else {
            (self.lower.as_str(), needle.to_ascii_lowercase())
        };
        let finder = memmem::Finder::new(needle.as_bytes());
        let mut matches = Vec::new();
        let mut spans = Vec::new();
        for start in finder.find_iter(hay.as_bytes()) {
            if matches.len() >= MAX_MATCHES {
                break;
            }
            let end = start + needle.len();
            // A hit may not include the newline between two logical lines.
            if hay.as_bytes()[start..end].contains(&b'\n') {
                continue;
            }
            let first = spans.len();
            self.spans_for(start, end, &mut spans);
            let count = spans.len() - first;
            if count > 0 {
                matches.push(Match { first, count, line: spans[first].line });
            }
        }
        (matches, spans)
    }

    /// The cell spans covering bytes `start..end` of the text.
    fn spans_for(&self, start: usize, end: usize, out: &mut Vec<Span>) {
        let r0 = self.rows.partition_point(|r| r.start <= start) - 1;
        let r1 = self.rows.partition_point(|r| r.start < end) - 1;
        for ri in r0..=r1 {
            let r = self.rows[ri];
            let from = if ri == r0 { self.cell_at(ri, start) } else { 0 };
            let to = if ri == r1 { self.cell_at(ri, end) } else { r.len };
            if to > from {
                out.push(Span { line: r.line, start: from, end: to });
            }
        }
    }

    /// The cell index of byte offset `at` within row `ri`: count the row's
    /// characters up to it, giving wide characters their two cells.
    fn cell_at(&self, ri: usize, at: usize) -> usize {
        let r = self.rows[ri];
        let text = &self.text[r.start..(at.min(self.text.len()))];
        let mut cell = 0;
        for c in text.chars() {
            cell += if unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) >= 2 { 2 } else { 1 };
        }
        cell.min(r.len)
    }
}

/// Arabic combining marks: harakat, shadda, sukun and the Quranic annotation
/// range. These live in cells as marks, not as text.
fn is_arabic_mark(c: char) -> bool {
    matches!(c as u32, 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7 | 0x06E8 | 0x06EA..=0x06ED)
}

/// Live state of the search bar.
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub matches: Vec<Match>,
    pub spans: Vec<Span>,
    /// Index into `matches` of the hit the view is on.
    pub current: Option<usize>,
}

impl Search {
    /// Spans of the current hit, for the stronger highlight.
    pub fn current_spans(&self) -> &[Span] {
        match self.current.and_then(|i| self.matches.get(i)) {
            Some(m) => &self.spans[m.first..m.first + m.count],
            None => &[],
        }
    }

    /// The first line of the current hit.
    pub fn current_line(&self) -> Option<usize> {
        self.current.and_then(|i| self.matches.get(i)).map(|m| m.line)
    }

    /// Re-run the query and land on the hit nearest `near` (a line), or the
    /// last one when there is no preference — search starts from the bottom.
    pub fn run(&mut self, index: &Index, near: Option<usize>) {
        let (matches, spans) = index.find(&self.query);
        self.matches = matches;
        self.spans = spans;
        self.current = if self.matches.is_empty() {
            None
        } else {
            match near {
                Some(line) => {
                    // The last hit at or above the line, else the first below.
                    let i = self.matches.partition_point(|m| m.line <= line);
                    Some(if i > 0 { i - 1 } else { 0 })
                }
                None => Some(self.matches.len() - 1),
            }
        };
    }

    /// Step to an older (`-1`) or newer (`+1`) hit, wrapping around.
    pub fn step(&mut self, dir: i32) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let cur = self.current.unwrap_or(n - 1) as i64;
        self.current = Some(((cur + dir as i64).rem_euclid(n as i64)) as usize);
    }
}
