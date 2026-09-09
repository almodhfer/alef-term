//! Text selection over the grid.

use crate::grid::Attrs;
use crate::term::Terminal;

/// A selection over **logical** grid positions.
///
/// Selection is logical, not visual: dragging across reordered Arabic selects
/// the text between the two endpoints as the program wrote it, which is what
/// gets copied and what a shell expects back. Within a uniform run that is also
/// visually contiguous, so it looks right as well.
///
/// Rows are **absolute line numbers** across scrollback and screen, not
/// viewport rows. Anchoring to the viewport would make a selection slide over
/// the text whenever output arrived or the view scrolled — it would end up
/// covering different characters than the ones that were dragged over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub block: bool,
}

impl Selection {
    /// Endpoints in order, regardless of which way the drag went.
    pub fn bounds(&self) -> ((usize, usize), (usize, usize)) {
        self.normalized()
    }

    fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    pub fn contains(&self, row: usize, cell: usize) -> bool {
        let (a, b) = self.normalized();
        if self.block {
            let (c0, c1) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
            return row >= a.0 && row <= b.0 && cell >= c0 && cell <= c1;
        }
        if row < a.0 || row > b.0 {
            return false;
        }
        if a.0 == b.0 {
            return cell >= a.1 && cell <= b.1;
        }
        if row == a.0 {
            return cell >= a.1;
        }
        if row == b.0 {
            return cell <= b.1;
        }
        true
    }
}


/// The text covered by a selection, in logical order.
///
/// Rows joined by a soft wrap are rejoined into one line, and trailing blanks
/// are trimmed, so pasting back what was copied reproduces the original
/// command rather than the way it happened to be broken across the screen.
pub fn selected_text(term: &Terminal, sel: Selection) -> String {
    let (a, b) = sel.bounds();
    let mut out = String::new();
    let mut pending_wrap = false;
    for abs_line in a.0..=b.0 {
        let Some(row) = term.grid.line(abs_line as isize) else { continue };
        let mut line = String::new();
        let mut any = false;
        for (i, cell) in row.cells.iter().enumerate() {
            if !sel.contains(abs_line, i) {
                continue;
            }
            any = true;
            if cell.attrs.contains(Attrs::WIDE_SPACER) {
                continue;
            }
            line.push(cell.ch);
            for m in cell.marks() {
                line.push(*m);
            }
        }
        if !any {
            continue;
        }
        if pending_wrap {
            // Previous row wrapped into this one: no newline between them.
        } else if !out.is_empty() {
            out.push('\n');
        }
        if row.wrapped {
            out.push_str(&line);
            pending_wrap = true;
        } else {
            out.push_str(line.trim_end());
            pending_wrap = false;
        }
    }
    out
}

/// A run of cells on one absolute line: `start..end`, end exclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Characters that end a word for double-click selection. Whitespace always
/// does; the rest are the shell's own punctuation, so a double click on a
/// path or an identifier takes the whole thing but not the quotes or
/// brackets around it.
const WORD_BREAKS: &str = " \t()[]{}<>'\"`,;|&=";

fn is_word(c: char) -> bool {
    !WORD_BREAKS.contains(c)
}

/// The word around `cell` on `line`, for a double click. `None` on blank.
pub fn word_at(term: &Terminal, line: usize, cell: usize) -> Option<Span> {
    let row = term.grid.line(line as isize)?;
    let cells = &row.cells;
    let at = cells.get(cell)?;
    if !is_word(at.ch) {
        return None;
    }
    let mut start = cell;
    while start > 0 && is_word(cells[start - 1].ch) {
        start -= 1;
    }
    let mut end = cell + 1;
    while end < cells.len() && is_word(cells[end].ch) {
        end += 1;
    }
    Some(Span { line, start, end })
}

/// The absolute lines of the logical line containing `line`: the row and the
/// rows soft-wrapped onto it either side. For a triple click.
pub fn logical_line(term: &Terminal, line: usize) -> (usize, usize) {
    let mut first = line;
    while first > 0 && term.grid.line(first as isize - 1).is_some_and(|r| r.wrapped) {
        first -= 1;
    }
    let mut last = line;
    while term.grid.line(last as isize).is_some_and(|r| r.wrapped)
        && term.grid.line(last as isize + 1).is_some()
    {
        last += 1;
    }
    (first, last)
}

/// Characters a URL may contain, once the scheme has been seen.
fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | '`' | '{' | '}' | '|' | '\\' | '^')
}

const SCHEMES: [&str; 5] = ["https://", "http://", "file://", "ftp://", "mailto:"];

/// A link under `cell` on `line`: an OSC 8 hyperlink the program attached, or
/// a bare URL in the text. Bare URLs are followed across soft wraps, since a
/// long one is usually broken by the margin.
pub fn url_at(term: &Terminal, line: usize, cell: usize) -> Option<(Span, String)> {
    let row = term.grid.line(line as isize)?;
    let at = row.cells.get(cell)?;
    if let Some(link) = at.extra.as_deref().and_then(|e| e.link.as_deref()) {
        // Extend over the neighbouring cells that carry the same target.
        let same = |c: &crate::grid::Cell| {
            c.extra.as_deref().and_then(|e| e.link.as_deref()) == Some(link)
        };
        let mut start = cell;
        while start > 0 && same(&row.cells[start - 1]) {
            start -= 1;
        }
        let mut end = cell + 1;
        while end < row.cells.len() && same(&row.cells[end]) {
            end += 1;
        }
        return Some((Span { line, start, end }, link.to_string()));
    }

    // Bare URL: gather the logical line as (line, cell, char) and look for a
    // scheme whose run of URL characters covers the clicked cell.
    let (first, last) = logical_line(term, line);
    let mut chars: Vec<(usize, usize, char)> = Vec::new();
    for l in first..=last {
        let Some(r) = term.grid.line(l as isize) else { break };
        let len = if r.wrapped { r.cells.len() } else { r.trimmed_len() };
        for (i, c) in r.cells[..len].iter().enumerate() {
            if !c.attrs.contains(Attrs::WIDE_SPACER) {
                chars.push((l, i, c.ch));
            }
        }
    }
    let here = chars.iter().position(|&(l, i, _)| l == line && i == cell)?;
    let text: String = chars.iter().map(|c| c.2).collect();
    // Byte offsets in `text` of each char, for matching schemes.
    let offsets: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();

    // The run of URL characters around the click.
    let mut a = here;
    while a > 0 && is_url_char(chars[a - 1].2) {
        a -= 1;
    }
    let mut b = here;
    while b < chars.len() && is_url_char(chars[b].2) {
        b += 1;
    }
    if a >= b {
        return None;
    }
    // The scheme may start anywhere in the run (e.g. after a `(`), so find the
    // last scheme at or before the click.
    let mut start = None;
    for i in a..=here {
        let rest = &text[offsets[i]..];
        if SCHEMES.iter().any(|s| rest.starts_with(s)) {
            start = Some(i);
        }
    }
    let start = start?;
    // Trailing punctuation is almost always the sentence's, not the URL's;
    // a closing bracket is kept only if the URL opened one.
    let mut end = b;
    loop {
        let Some(&(_, _, c)) = chars.get(end - 1) else { break };
        let opened = |o: char| chars[start..end - 1].iter().filter(|x| x.2 == o).count();
        let closed = |k: char| chars[start..end - 1].iter().filter(|x| x.2 == k).count();
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => closed(')') >= opened('('),
            ']' => closed(']') >= opened('['),
            _ => false,
        };
        if drop && end - 1 > start {
            end -= 1;
        } else {
            break;
        }
    }
    if here >= end {
        return None;
    }
    let url: String = chars[start..end].iter().map(|c| c.2).collect();
    if url.len() <= SCHEMES.iter().find(|s| url.starts_with(*s)).map_or(0, |s| s.len()) {
        return None;
    }
    // The span reported is the part on the clicked row, for underlining.
    let on_row: Vec<usize> = chars[start..end].iter().filter(|c| c.0 == line).map(|c| c.1).collect();
    let span = Span { line, start: *on_row.first()?, end: on_row.last()? + 1 };
    Some((span, url))
}
