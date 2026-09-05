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
    /// Logical cell this glyph came from. Colour and attributes are read from
    /// that cell at blit time rather than stored here, so a colour change costs
    /// no reshaping and a cached layout stays valid across one.
    pub cell: usize,
}

/// Text surrounding a row, used only to give the shaper joining context.
///
/// When a line soft-wraps mid-word the letters at the break still have to take
/// their joined forms — the word did not end, the display line did. Passing the
/// neighbouring text as shaping context gets the contextual forms right without
/// laying the paragraph out as one unit.
#[derive(Clone, Copy, Default)]
pub struct RowContext<'a> {
    /// Text logically preceding this row (tail of the row that wrapped into it).
    pub pre: &'a str,
    /// Text logically following this row (head of the row it wraps into).
    pub post: &'a str,
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

/// How much surrounding text to hand the shaper as joining context. A cursive
/// form depends only on its immediate neighbours, so a couple of characters is
/// plenty.
const CONTEXT_CHARS: usize = 2;

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
    pre_context: String,
    post_context: String,
}

impl Layouter {
    pub fn new() -> Self {
        Layouter {
            text: String::new(),
            items: Vec::new(),
            buf: Some(rustybuzz::UnicodeBuffer::new()),
            pre_context: String::new(),
            post_context: String::new(),
        }
    }

    pub fn layout_row(
        &mut self,
        row: &Row,
        cols: usize,
        fonts: &mut Fonts,
        mode: BidiMode,
        bidi_enabled: bool,
    ) -> RowLayout {
        self.layout_row_ctx(row, cols, fonts, mode, bidi_enabled, RowContext::default())
    }

    pub fn layout_row_ctx(
        &mut self,
        row: &Row,
        cols: usize,
        fonts: &mut Fonts,
        mode: BidiMode,
        bidi_enabled: bool,
        ctx: RowContext<'_>,
    ) -> RowLayout {
        self.pre_context.clear();
        self.pre_context.push_str(ctx.pre);
        self.post_context.clear();
        self.post_context.push_str(ctx.post);
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

        let cell_w = fonts.cell_w;

        // Column span of each group, so it can be anchored independently.
        let mut spans: Vec<(f32, f32)> = Vec::new();
        {
            let mut x = box_x;
            for g in &groups {
                let w = g.items.iter().map(|&i| self.items[i].columns).sum::<usize>() as f32
                    * cell_w;
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

            let first = *g.items.first().unwrap();
            let last = *g.items.last().unwrap();
            let cursive = g.slot == Slot::Arabic;

            // Joining context. A cursive letter's form depends on its
            // neighbours, so a group has to be shaped knowing what sits either
            // side of it — the next display row after a soft wrap, or the other
            // half of a word split across font runs.
            //
            // rustybuzz has set_pre_context/set_post_context, but they do not
            // reach its Arabic joining pass: shaping "بالع" with "الم" as
            // post-context still yields the final form of ع. So the context is
            // shaped *with* the text and its glyphs are dropped afterwards,
            // which is the sequence the shaper would really see.
            let ctx_base = self.items.len() as u32 + 1;
            let mut pre_ctx = String::new();
            let mut post_ctx = String::new();
            if cursive {
                for i in first.saturating_sub(CONTEXT_CHARS)..first {
                    if let Some(it) = self.items.get(i) {
                        pre_ctx.push(it.ch);
                    }
                }
                if pre_ctx.is_empty() {
                    pre_ctx.push_str(&self.pre_context);
                }
                for i in last + 1..(last + 1 + CONTEXT_CHARS).min(self.items.len()) {
                    post_ctx.push(self.items[i].ch);
                }
                if post_ctx.is_empty() {
                    post_ctx.push_str(&self.post_context);
                }
            }

            for c in pre_ctx.chars() {
                buf.add(c, ctx_base);
            }

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
            for c in post_ctx.chars() {
                buf.add(c, ctx_base);
            }

            buf.set_direction(if rtl {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            });
            if cursive {
                buf.set_script(rustybuzz::script::ARABIC);
            }
            let shaped = rustybuzz::shape(&fonts.faces[font].hb, &[], buf);

            let infos = shaped.glyph_infos();
            let poss = shaped.glyph_positions();

            // Drop the context glyphs, keeping the pen offsets the real glyphs
            // were shaped at so their relative positions survive.
            let is_ctx = |k: usize| infos[k].cluster >= ctx_base;
            let mut kept: Vec<(usize, f32)> = Vec::with_capacity(infos.len());
            let mut walk = 0.0f32;
            for k in 0..infos.len() {
                if !is_ctx(k) {
                    kept.push((k, walk));
                }
                walk += poss[k].x_advance as f32 * scale;
            }
            if kept.is_empty() {
                self.buf = Some(shaped.clear());
                continue;
            }
            let origin = kept[0].1;
            let natural: f32 =
                kept.iter().map(|&(k, _)| poss[k].x_advance as f32 * scale).sum();

            let (span_x, span_w) = spans[gi];

            // Non-cursive scripts are snapped to the grid, one cluster per
            // cell. This is not optional: `cell_w` is rounded to a whole pixel
            // (Menlo advances 9.64px into a 10px cell), so laying a long Latin
            // run out with the font's own advances drifts about a third of a
            // pixel per character — nearly a full cell across an 80-column
            // status bar, which makes characters collide.
            //
            // Arabic is exempt because snapping is exactly what tears cursive
            // joins apart: a word has to be placed as one piece.
            if !cursive {
                let mut i = 0usize;
                while i < kept.len() {
                    let cl = infos[kept[i].0].cluster;
                    let mut j = i;
                    let mut adv = 0.0f32;
                    while j < kept.len() && infos[kept[j].0].cluster == cl {
                        adv += poss[kept[j].0].x_advance as f32 * scale;
                        j += 1;
                    }
                    let item = self.items.get(cl as usize).unwrap_or(&self.items[g.items[0]]);
                    let col = out.cell_to_column.get(item.cell).copied().unwrap_or(item.cell);
                    // A cluster can cover several items — a ligature like `->`
                    // fuses two characters into one glyph — so its box is all
                    // the columns those items own, not just the first one's.
                    // Measuring only the first would squeeze the glyph into a
                    // single cell and overlap its neighbour.
                    let next_cl = kept
                        .get(j)
                        .map(|&(n, _)| infos[n].cluster as usize)
                        .unwrap_or(*g.items.last().unwrap() + 1);
                    let span_cols: usize = (cl as usize..next_cl.max(cl as usize + 1))
                        .filter_map(|ix| self.items.get(ix))
                        .map(|it| it.columns)
                        .sum();
                    let cell_span = span_cols.max(item.columns) as f32 * cell_w;
                    // Centre the cluster in its cell so the rounding slack is
                    // split evenly rather than pushed to one side.
                    let mut pen = col as f32 * cell_w + (cell_span - adv) * 0.5;
                    for &(k, _) in &kept[i..j] {
                        let inkless = item.ch == ' ' && item.marks.is_empty();
                        if !inkless && !crate::boxdraw::is_procedural(item.ch) {
                            out.glyphs.push(PositionedGlyph {
                                font,
                                glyph: infos[k].glyph_id as u16,
                                x: pen + poss[k].x_offset as f32 * scale,
                                y: poss[k].y_offset as f32 * scale,
                                px,
                                cell: item.cell,
                            });
                        }
                        pen += poss[k].x_advance as f32 * scale;
                    }
                    i = j;
                }
                self.buf = Some(shaped.clear());
                continue;
            }

            // If a cursive group's ink is wider than the columns it owns,
            // shrink it *isotropically* to fit. Squeezing only the x axis would
            // distort the letterforms and tear the joins; scaling uniformly
            // keeps the word's shape and just sets it slightly smaller. Letting
            // it overhang is not an option between words — the neighbour's ink
            // would start before this one ends and swallow the space.
            let (scale, px, natural, shrink) = if natural > span_w && natural > 0.0 {
                let k = span_w / natural;
                (scale * k, px * k, span_w, k)
            } else {
                (scale, px, natural, 1.0f32)
            };

            // Anchor at the group's leading edge: right for RTL, left for LTR.
            // Advances are otherwise used verbatim, so GPOS mark anchoring
            // places harakat exactly where the font intends.
            let base = if rtl { span_x + (span_w - natural).max(0.0) } else { span_x };

            for &(k, at) in &kept {
                let src = infos[k].cluster as usize;
                let item = self.items.get(src).unwrap_or(&self.items[g.items[0]]);
                let inkless = item.ch == ' ' && item.marks.is_empty();
                if !inkless && !crate::boxdraw::is_procedural(item.ch) {
                    out.glyphs.push(PositionedGlyph {
                        font,
                        glyph: infos[k].glyph_id as u16,
                        x: base + (at - origin) * shrink + poss[k].x_offset as f32 * scale,
                        y: poss[k].y_offset as f32 * scale,
                        px,
                        cell: item.cell,
                    });
                }
            }
            self.buf = Some(shaped.clear());
        }
    }
}
