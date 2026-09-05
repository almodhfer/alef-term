//! Turning a logical row of cells into positioned glyphs.
//!
//! This is where Arabic actually gets handled. A row arrives in logical order;
//! we resolve it with the Unicode Bidirectional Algorithm, split it into runs
//! of uniform direction and font, shape each run whole with rustybuzz (so
//! letters join across cell boundaries), and place the glyphs into the columns
//! the run owns.
//!
//! Two invariants keep full-screen programs working:
//!
//! 1. **The visual column grid is strictly uniform.** Column `v` always covers
//!    pixels `[v*cell_w, (v+1)*cell_w)`. Bidi is a *permutation of cells onto
//!    columns*, never a change of column widths. Backgrounds, selection, the
//!    cursor and hit-testing all read the grid, so they stay exact.
//! 2. **Glyph ink inside that grid is free.** Advances come from the font and
//!    are never scaled; ink may overhang its columns. Squeezing glyphs to fit
//!    a cell is what tears Arabic joins apart, and it buys nothing, because
//!    nothing downstream measures ink.

use crate::color::Color;
use crate::font::{Fonts, Slot, Style};
use crate::grid::{Attrs, Row};
use crate::term::BidiMode;
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Copy, Debug)]
pub struct PositionedGlyph {
    pub font: usize,
    pub glyph: u16,
    /// Pen x in pixels, relative to the left edge of the row.
    pub x: f32,
    /// Baseline-relative y offset (positive moves the glyph up).
    pub y: f32,
    pub px: f32,
    pub fg: Color,
    pub attrs: Attrs,
    /// Logical cell this glyph came from, for hit-testing and selection.
    pub cell: usize,
}

#[derive(Default)]
pub struct RowLayout {
    pub glyphs: Vec<PositionedGlyph>,
    /// Logical cell index occupying each visual column.
    pub visual_cells: Vec<usize>,
    /// Leading visual column for each logical cell.
    pub cell_to_column: Vec<usize>,
    /// True when any run in the row resolved to RTL.
    pub rtl: bool,
}

impl RowLayout {
    /// Visual column a logical cell is drawn at.
    pub fn column_of(&self, cell: usize) -> usize {
        self.cell_to_column.get(cell).copied().unwrap_or(cell)
    }
    /// Logical cell drawn at a visual column.
    pub fn cell_at(&self, col: usize) -> usize {
        self.visual_cells.get(col).copied().unwrap_or(col)
    }
}

/// One character extracted from the row, with its provenance.
struct Item {
    ch: char,
    marks: Vec<char>,
    cell: usize,
    columns: usize,
    /// Byte offset of this item within the bidi analysis text.
    bstart: usize,
}

/// Explicit bidi controls and joiners. These take part in bidi resolution but
/// must never reach the shaper, which would emit `.notdef` boxes for them.
#[inline]
fn is_bidi_control(c: char) -> bool {
    matches!(c as u32,
        0x200E | 0x200F | 0x061C | 0x202A..=0x202E | 0x2066..=0x2069)
}

pub struct Layouter {
    /// Scratch reused across rows to keep per-frame allocation down.
    text: String,
    items: Vec<Item>,
    buf: Option<rustybuzz::UnicodeBuffer>,
}

impl Layouter {
    pub fn new() -> Self {
        Layouter { text: String::new(), items: Vec::new(), buf: Some(rustybuzz::UnicodeBuffer::new()) }
    }

    pub fn layout_row(
        &mut self,
        row: &Row,
        cols: usize,
        fonts: &mut Fonts,
        mode: BidiMode,
        bidi_enabled: bool,
    ) -> RowLayout {
        self.items.clear();

        // --- 1. Collect logical characters. The trailing half of a wide
        // character contributes nothing: it is a column, not a character, and
        // its blank would sever the joining chain running through it.
        for (i, cell) in row.cells.iter().take(cols).enumerate() {
            if cell.attrs.contains(Attrs::WIDE_SPACER) {
                continue;
            }
            let columns = if cell.attrs.contains(Attrs::WIDE_LEADING) { 2 } else { 1 };
            self.items.push(Item {
                ch: cell.ch,
                marks: cell.marks().to_vec(),
                cell: i,
                columns,
                bstart: 0,
            });
        }

        // Trailing blanks carry no ink, and feeding them to the UBA lets rule
        // L1 pull them into the reordering. They are painted as background at
        // their own columns instead.
        while matches!(self.items.last(), Some(it) if it.ch == ' ' && it.marks.is_empty()) {
            self.items.pop();
        }

        let mut out = RowLayout::default();
        out.cell_to_column = vec![0; row.cells.len()];
        for c in 0..cols {
            out.visual_cells.push(c);
        }
        for (c, slot) in out.cell_to_column.iter_mut().enumerate() {
            *slot = c;
        }
        if self.items.is_empty() {
            return out;
        }

        // --- 2. Field segmentation. Reordering is confined inside a segment;
        // segments themselves stay in logical order at fixed columns.
        //
        // A run of two or more blanks, and any box-drawing character, is a
        // hard barrier. This is what stops a single Arabic word from dragging
        // a table's column separator — or a whole `ls` column — across the
        // screen, while still letting the text inside each field reorder.
        let mut segments: Vec<(usize, usize, bool)> = Vec::new(); // (start, end, reorder)
        {
            let n = self.items.len();
            let is_barrier_char =
                |it: &Item| crate::boxdraw::is_procedural(it.ch) || it.ch == '\t';
            let mut i = 0;
            while i < n {
                let it = &self.items[i];
                let blank_run = if it.ch == ' ' && it.marks.is_empty() {
                    let mut j = i;
                    while j < n && self.items[j].ch == ' ' && self.items[j].marks.is_empty() {
                        j += 1;
                    }
                    j - i
                } else {
                    0
                };
                if is_barrier_char(it) || blank_run >= 2 {
                    let end = if blank_run >= 2 { i + blank_run } else { i + 1 };
                    segments.push((i, end, false));
                    i = end;
                } else {
                    let start = i;
                    while i < n {
                        let it = &self.items[i];
                        if is_barrier_char(it) {
                            break;
                        }
                        if it.ch == ' ' && it.marks.is_empty() {
                            let mut j = i;
                            while j < n && self.items[j].ch == ' ' && self.items[j].marks.is_empty()
                            {
                                j += 1;
                            }
                            if j - i >= 2 {
                                break;
                            }
                            i = j;
                        } else {
                            i += 1;
                        }
                    }
                    segments.push((start, i, true));
                }
            }
        }

        // --- 3. Walk segments left to right, assigning columns as we go.
        let cell_w = fonts.cell_w;
        let mut column = 0usize;

        for (s, e, reorderable) in segments {
            if !reorderable || !bidi_enabled {
                // Barrier: strictly logical order, one item per its columns.
                for i in s..e {
                    let it = &self.items[i];
                    for k in 0..it.columns {
                        if column + k < cols {
                            out.visual_cells[column + k] = it.cell + k;
                        }
                    }
                    if it.cell < out.cell_to_column.len() {
                        out.cell_to_column[it.cell] = column;
                    }
                    column += it.columns;
                }
                let idxs: Vec<usize> = (s..e).collect();
                let start_col = column - idxs.iter().map(|&i| self.items[i].columns).sum::<usize>();
                self.shape_run(
                    &idxs,
                    false,
                    start_col as f32 * cell_w,
                    (column - start_col) as f32 * cell_w,
                    row,
                    fonts,
                    &mut out,
                );
                continue;
            }

            // --- Bidi within the segment. The base level is a *policy*, not a
            // guess: see BidiMode.
            self.text.clear();
            for i in s..e {
                self.items[i].bstart = self.text.len();
                let it = &self.items[i];
                self.text.push(it.ch);
                for m in &it.marks {
                    self.text.push(*m);
                }
            }
            let base = match mode {
                BidiMode::Auto => None,
                BidiMode::ForceLtr => Some(Level::ltr()),
                BidiMode::ForceRtl => Some(Level::rtl()),
            };
            let info = ParagraphBidiInfo::new(&self.text, base);
            let (levels, runs) = info.visual_runs(0..self.text.len());

            for run in &runs {
                let rtl = levels.get(run.start).map_or(false, |l| l.is_rtl());
                out.rtl |= rtl;

                let idxs: Vec<usize> = (s..e)
                    .filter(|&i| self.items[i].bstart >= run.start && self.items[i].bstart < run.end)
                    .collect();
                if idxs.is_empty() {
                    continue;
                }
                let run_columns: usize = idxs.iter().map(|&i| self.items[i].columns).sum();

                // Within a uniform-level RTL run, logical order runs
                // right-to-left across the columns it owns.
                let mut visual = idxs.clone();
                if rtl {
                    visual.reverse();
                }
                let mut c = column;
                for &i in &visual {
                    let it = &self.items[i];
                    for k in 0..it.columns {
                        if c + k < cols {
                            out.visual_cells[c + k] = it.cell + k;
                        }
                    }
                    if it.cell < out.cell_to_column.len() {
                        out.cell_to_column[it.cell] = c;
                    }
                    c += it.columns;
                }

                self.shape_run(
                    &idxs,
                    rtl,
                    column as f32 * cell_w,
                    run_columns as f32 * cell_w,
                    row,
                    fonts,
                    &mut out,
                );
                column += run_columns;
            }
        }

        out
    }

    /// Shape one run and place its glyphs across `[box_x, box_x + box_w]`.
    ///
    /// `idxs` is in **logical** order. rustybuzz expects its buffer in logical
    /// order and emits glyphs already reversed for a right-to-left buffer, so
    /// the characters must not be pre-reversed here. Only the order of the
    /// groups across the screen flips for RTL.
    fn shape_run(
        &mut self,
        idxs: &[usize],
        rtl: bool,
        box_x: f32,
        box_w: f32,
        row: &Row,
        fonts: &mut Fonts,
        out: &mut RowLayout,
    ) {
        // Split into groups of uniform font. We deliberately do NOT split on
        // colour: a colour change mid-word must not break Arabic joining, so
        // each glyph takes its colour from the cell it came from instead.
        //
        // We also break at spaces, because a space ends a cursive joining
        // group. Anchoring each group separately means the slack between two
        // words lands in the gap between them, where it belongs, instead of
        // pooling at one end of the line.
        struct Group {
            slot: Slot,
            style: Style,
            items: Vec<usize>,
        }
        let mut groups: Vec<Group> = Vec::new();
        for &i in idxs {
            let it = &self.items[i];
            let cell = &row.cells[it.cell];
            let style = Style {
                bold: cell.attrs.contains(Attrs::BOLD),
                italic: cell.attrs.contains(Attrs::ITALIC),
            };
            let slot = fonts.slot_for(it.ch, style);
            let breaks = it.ch == ' ';
            match groups.last_mut() {
                Some(g) if g.slot == slot && g.style == style && !breaks => g.items.push(i),
                _ => groups.push(Group { slot, style, items: vec![i] }),
            }
            if breaks {
                // force the next character to open a fresh group
                groups.push(Group { slot, style, items: Vec::new() });
            }
        }
        groups.retain(|g| !g.items.is_empty());
        // Groups were built in logical order; on screen an RTL run lays them
        // out right to left. Reversing the group *list* (never the characters
        // inside a group) puts them in visual order for span assignment.
        if rtl {
            groups.reverse();
        }

        // Column span of each group, so it can be anchored independently.
        let mut spans: Vec<(f32, f32)> = Vec::new();
        {
            let mut x = box_x;
            for g in &groups {
                let w = g.items.iter().map(|&i| self.items[i].columns).sum::<usize>() as f32
                    * fonts.cell_w;
                spans.push((x, w));
                x += w;
            }
        }
        let _ = box_w;

        for (gi, g) in groups.iter().enumerate() {
            let font = fonts.face_id(g.slot, g.style);
            let px = fonts.px_for(g.slot);
            let upem = fonts.faces[font].upem;
            let scale = px / upem;

            let mut buf = self.buf.take().unwrap_or_default();
            buf.clear();
            // Cluster ids are item indices, so glyphs map straight back to
            // cells without a byte-offset table. Bidi controls take part in
            // resolution but are never shaped.
            //
            // Mirroring of paired brackets (UAX#9 L4) is left entirely to
            // rustybuzz, which applies it for right-to-left buffers. Doing it
            // here as well would cancel it back out.
            let mut any = false;
            for &i in &g.items {
                let it = &self.items[i];
                if is_bidi_control(it.ch) {
                    continue;
                }
                buf.add(it.ch, i as u32);
                any = true;
                for m in &it.marks {
                    buf.add(*m, i as u32);
                }
            }
            if !any {
                self.buf = Some(buf);
                continue;
            }
            buf.set_direction(if rtl {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            });
            let shaped = rustybuzz::shape(&fonts.faces[font].hb, &[], buf);

            let infos = shaped.glyph_infos();
            let poss = shaped.glyph_positions();
            let natural: f32 = poss.iter().map(|p| p.x_advance as f32 * scale).sum();

            let (span_x, span_w) = spans[gi];

            // If the group's ink is wider than the columns it owns, shrink it
            // *isotropically* to fit. Squeezing only the x axis would distort
            // the letterforms and tear the joins; scaling uniformly keeps the
            // word's shape and just sets it slightly smaller. Letting it
            // overhang instead is not an option between words — the neighbour's
            // ink would start before this one ends and swallow the space.
            let (scale, px, natural) = if natural > span_w && natural > 0.0 {
                let k = span_w / natural;
                (scale * k, px * k, span_w)
            } else {
                (scale, px, natural)
            };

            // Anchor at the group's leading edge: right for RTL, left for LTR.
            // Advances are otherwise used verbatim, so GPOS mark anchoring
            // places harakat exactly where the font intends.
            let mut pen = if rtl { span_x + (span_w - natural).max(0.0) } else { span_x };

            for (info, p) in infos.iter().zip(poss) {
                let src = info.cluster as usize;
                let item = self.items.get(src).unwrap_or(&self.items[g.items[0]]);
                let cell = &row.cells[item.cell];
                let inkless = item.ch == ' ' && item.marks.is_empty();
                if !inkless && !crate::boxdraw::is_procedural(item.ch) {
                    out.glyphs.push(PositionedGlyph {
                        font,
                        glyph: info.glyph_id as u16,
                        x: pen + p.x_offset as f32 * scale,
                        y: p.y_offset as f32 * scale,
                        px,
                        fg: cell.fg,
                        attrs: cell.attrs,
                        cell: item.cell,
                    });
                }
                pen += p.x_advance as f32 * scale;
            }
            self.buf = Some(shaped.clear());
        }
    }
}
