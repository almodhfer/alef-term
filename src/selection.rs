//! Text selection over the grid.

use crate::grid::Attrs;
use crate::term::Terminal;

/// A selection over **logical** grid positions.
///
/// Selection is logical, not visual: dragging across reordered Arabic selects
/// the text between the two endpoints as the program wrote it, which is what
/// gets copied and what a shell expects back. Within a uniform run that is also
/// visually contiguous, so it looks right as well.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub block: bool,
}

impl Selection {
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
    let mut out = String::new();
    let mut pending_wrap = false;
    for r in 0..term.grid.rows {
        let row = term.grid.viewport_row(r);
        let mut line = String::new();
        let mut any = false;
        for (i, cell) in row.cells.iter().enumerate() {
            if !sel.contains(r, i) {
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
