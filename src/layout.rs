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
use unicode_bidi::{BidiClass, Level, ParagraphBidiInfo};

#[derive(Clone, Copy, Debug)]
pub struct PositionedGlyph {
    pub font: usize,
    pub glyph: u16,
    /// Pen x in pixels, relative to the left edge of the row.
    pub x: f32,
    /// Baseline-relative y offset (positive moves the glyph up).
    pub y: f32,
    pub px: f32,
    /// Horizontal scale of the outline; below 1 for a word condensed to fit.
    pub stretch: f32,
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
    /// Direction of each logical cell; a Latin cell in a mixed row is LTR.
    rtl_cells: Vec<bool>,
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

    pub fn is_rtl(&self, cell: usize) -> bool {
        self.rtl_cells.get(cell).copied().unwrap_or(false)
    }
}

/// A terminal field can be prose or shell syntax. Keep recognizable prompt
/// sigils and paths anchored, while applying P2/P3 to ordinary sentences.
/// This is deliberately small: ambiguous output can use the forced modes.
fn auto_base(text: &str) -> Option<Level> {
    let first = text.split_whitespace().next().unwrap_or("");
    let prompt = first.starts_with(['$', '%', '#', '>', '❯', '➜']);
    let path = first.starts_with(['/', '~', '.']) || first.contains('/')
        || first.rsplit_once('.').is_some_and(|(_, ext)| {
            !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    if prompt || path {
        return Some(Level::ltr());
    }
    // An English sentence that merely opens with an Arabic word — a glossary
    // entry, "- حفيف (Hafeef): the rustle of leaves" — is still an English
    // sentence. First-strong would flip the whole line, so the script that
    // carries most of the letters wins; a tie falls back to P2/P3.
    let (mut ltr, mut rtl) = (0usize, 0usize);
    for c in text.chars() {
        match unicode_bidi::bidi_class(c) {
            BidiClass::L => ltr += 1,
            BidiClass::R | BidiClass::AL => rtl += 1,
            _ => {}
        }
    }
    (ltr > rtl).then(Level::ltr)
}

/// Split the field `items[ts..te]` into `(start, end, reorder)` segments,
/// keeping the parts that are structure rather than text where they were sent.
///
/// Two shapes are recognised, and only in a field that holds right-to-left
/// text at all:
///
/// - An editor's line number: digits opening the row or a pane beside a file
///   tree, one blank, then text in a different colour. Left in the paragraph,
///   an Arabic line would carry its own line number to the far end of the text.
/// - A delimited record, `903,عنوان,مؤلف,2014`: digits and a delimiter with no
///   blank after it. The delimiters then separate fields the way a table's
///   borders do, so the columns keep their order and each reorders inside
///   itself. A delimiter followed by a blank is punctuation and stays text.
///
/// An Arabic sentence that merely opens with a number — "3 كتب", "1. أولا" —
/// matches neither and remains one right-to-left paragraph.
fn structured_fields(
    items: &[Item],
    row: &Row,
    mut ts: usize,
    te: usize,
    leads: bool,
    out: &mut Vec<(usize, usize, bool)>,
) {
    let blank = |it: &Item| it.ch == ' ' && it.marks.is_empty();
    let rtl = |it: &Item| matches!(unicode_bidi::bidi_class(it.ch), BidiClass::R | BidiClass::AL);
    if !items[ts..te].iter().any(rtl) {
        out.push((ts, te, true));
        return;
    }
    let digits = |from: usize| items[from..te].iter().take_while(|it| it.ch.is_ascii_digit()).count();

    let d = digits(ts);
    if leads && d > 0 && ts + d + 1 < te && blank(&items[ts + d]) {
        let look = |i: usize| {
            let cell = &row.cells[items[i].cell];
            (cell.fg, cell.attrs.contains(Attrs::DIM))
        };
        if look(ts) != look(ts + d + 1) {
            out.push((ts, ts + d + 1, false));
            ts += d + 1;
        }
    }

    let d = digits(ts);
    if d > 0 && ts + d + 1 < te {
        let delim = items[ts + d].ch;
        let next = &items[ts + d + 1];
        if [',', ';', '|', ':'].contains(&delim) && !next.ch.is_ascii_digit() && !blank(next) {
            out.push((ts, ts + d + 1, false));
            ts += d + 1;
            // A colon introduces the rest of the line (`grep -n`); the others
            // separate every field of the record.
            if delim != ':' {
                for i in ts..te - 1 {
                    if items[i].ch == delim && !blank(&items[i + 1]) {
                        if ts < i {
                            out.push((ts, i, true));
                        }
                        out.push((i, i + 1, false));
                        ts = i + 1;
                    }
                }
            }
        }
    }
    if ts < te {
        out.push((ts, te, true));
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
        out.rtl_cells = vec![false; row.cells.len()];
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
                    // A lone blank at either edge of a field is its padding
                    // against the barrier, not part of the text. Left inside,
                    // an RTL field would carry it to the far side and sit
                    // flush against the table border.
                    let blank = |it: &Item| it.ch == ' ' && it.marks.is_empty();
                    let (mut ts, mut te) = (start, i);
                    while ts < te && blank(&self.items[ts]) {
                        ts += 1;
                    }
                    while te > ts && blank(&self.items[te - 1]) {
                        te -= 1;
                    }
                    if start < ts {
                        segments.push((start, ts, false));
                    }
                    if ts < te {
                        if bidi_enabled && mode == BidiMode::Auto {
                            // The field opens a row or a pane: nothing but
                            // blanks back to the margin or a border.
                            let leads = self.items[..ts].iter().rev()
                                .find(|it| !blank(it))
                                .map_or(true, |it| is_barrier_char(it));
                            structured_fields(&self.items, row, ts, te, leads, &mut segments);
                        } else {
                            segments.push((ts, te, true));
                        }
                    }
                    if te < i {
                        segments.push((te, i, false));
                    }
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
                        if it.cell + k < out.cell_to_column.len() {
                            out.cell_to_column[it.cell + k] = column + k;
                        }
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
                BidiMode::Auto => auto_base(&self.text),
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
                        if it.cell + k < out.cell_to_column.len() {
                            out.cell_to_column[it.cell + k] = c + k;
                            out.rtl_cells[it.cell + k] = rtl;
                        }
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
            /// The face; one slot may hold several when fallback fonts are loaded.
            font: usize,
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
            let (slot, font) = fonts.pick(it.ch, style);
            let breaks = it.ch == ' ';
            match groups.last_mut() {
                Some(g) if g.slot == slot && g.font == font && g.style == style && !breaks => g.items.push(i),
                _ => groups.push(Group { slot, font, style, items: vec![i] }),
            }
            if breaks {
                // force the next character to open a fresh group
                groups.push(Group { slot, font, style, items: Vec::new() });
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

        // Groups are visited in logical order, so a word is placed before the
        // punctuation that follows it. `trailing` is the last item of the
        // cursive group just placed and the slack left at its trailing edge.
        let mut trailing: Option<(usize, f32)> = None;
        // Cursive words collect here until their phrase ends; see `place_phrase`.
        let room = (fonts.baseline, fonts.cell_h - fonts.baseline);
        let mut phrase: Vec<Word> = Vec::new();
        let mut gap_pending = false;
        let order: Vec<usize> =
            if rtl { (0..groups.len()).rev().collect() } else { (0..groups.len()).collect() };
        for gi in order {
            let g = &groups[gi];
            let font = g.font;
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
            let face = &fonts.faces[font].hb;
            let plan = fonts.plans.get(face, font, &mut buf, cursive || fonts.ligatures);
            let shaped = rustybuzz::shape_with_plan(face, plan, buf);

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
                // A single blank between two words continues the phrase;
                // anything else ends it.
                let it = &self.items[first];
                let lone_blank = g.items.len() == 1 && it.ch == ' ' && it.marks.is_empty();
                if rtl && lone_blank && !phrase.is_empty() && !gap_pending {
                    gap_pending = true;
                } else {
                    trailing = place_phrase(&mut phrase, out, room, cell_w);
                    gap_pending = false;
                }
                // A cursive word rarely fills the cells it owns, and the slack
                // sits at its trailing edge — exactly where closing punctuation
                // goes. Centred in its own cell, a bracket or comma would stand
                // a visible gap away from the word it closes, so it is drawn
                // against the word's ink instead.
                let (pull, pull_by) = match trailing.take() {
                    Some((prev, slack)) if rtl && first == prev + 1 => {
                        let closes = |&&i: &&usize| {
                            let it = &self.items[i];
                            it.marks.is_empty()
                                && matches!(it.ch, ')' | ']' | '}' | ',' | '.' | ';' | ':' | '!' | '?' | '»' | '"' | '\'')
                        };
                        (g.items.iter().take_while(closes).count(), slack)
                    }
                    _ => (0, 0.0),
                };
                // Glyph order is visual; cluster IDs remain logical. The next
                // glyph therefore has a *smaller* cluster ID in RTL runs.
                let mut boundaries: Vec<usize> = kept.iter()
                    .map(|&(k, _)| infos[k].cluster as usize).collect();
                boundaries.sort_unstable();
                boundaries.dedup();
                boundaries.push(*g.items.last().unwrap() + 1);
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
                    // A cluster can cover several items — a ligature like `->`
                    // fuses two characters into one glyph — so its box is all
                    // the columns those items own, not just the first one's.
                    // Measuring only the first would squeeze the glyph into a
                    // single cell and overlap its neighbour.
                    let next_cl = boundaries[boundaries.binary_search(&(cl as usize)).unwrap() + 1];
                    let start = g.items.partition_point(|&ix| ix < cl as usize);
                    let end = g.items.partition_point(|&ix| ix < next_cl);
                    let mut col = out.column_of(item.cell);
                    let mut span_cols = 0;
                    for &ix in &g.items[start..end] {
                        let it = &self.items[ix];
                        col = col.min(out.column_of(it.cell));
                        span_cols += it.columns;
                    }
                    let cell_span = span_cols.max(item.columns) as f32 * cell_w;
                    // Centre the cluster in its cell so the rounding slack is
                    // split evenly rather than pushed to one side.
                    let mut pen = col as f32 * cell_w + (cell_span - adv) * 0.5;
                    if start < pull {
                        pen += pull_by;
                    }
                    // An icon is sized to the room it has, whichever cut of the
                    // font supplies it: about two cells when a blank follows,
                    // its own cell when text does.
                    if j == i + 1 && !rtl && item.columns == 1 && item.marks.is_empty() && is_icon(item.ch) {
                        let roomy = self.items.get(cl as usize + 1)
                            .map_or(true, |next| next.ch == ' ' && next.marks.is_empty());
                        let glyph = infos[kept[i].0].glyph_id as u16;
                        if let Some(icon) = fit_icon(fonts, font, glyph, px, col as f32 * cell_w, cell_w, roomy) {
                            out.glyphs.push(PositionedGlyph { cell: item.cell, ..icon });
                            i = j;
                            continue;
                        }
                    }
                    // A Powerline separator continues the cells beside it, so
                    // it spans the row's full height like the blocks do.
                    if j == i + 1 && matches!(item.ch, '\u{e0b0}'..='\u{e0d7}') {
                        let k = kept[i].0;
                        if let Some(edge) = fit_separator(fonts, font, infos[k].glyph_id as u16, px, pen) {
                            out.glyphs.push(PositionedGlyph { cell: item.cell, ..edge });
                            i = j;
                            continue;
                        }
                    }
                    for &(k, _) in &kept[i..j] {
                        let inkless = item.ch == ' ' && item.marks.is_empty();
                        if !inkless && !crate::boxdraw::is_procedural(item.ch) {
                            out.glyphs.push(PositionedGlyph {
                                font,
                                glyph: infos[k].glyph_id as u16,
                                x: pen + poss[k].x_offset as f32 * scale,
                                y: poss[k].y_offset as f32 * scale,
                                px,
                                stretch: 1.0,
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

            // A cursive word is not placed yet. It joins the phrase it belongs
            // to, and the phrase is set as a whole once it is complete.
            let glyphs = kept.iter().filter_map(|&(k, at)| {
                let src = infos[k].cluster as usize;
                let item = self.items.get(src).unwrap_or(&self.items[g.items[0]]);
                let inkless = item.ch == ' ' && item.marks.is_empty();
                (!inkless && !crate::boxdraw::is_procedural(item.ch)).then(|| WordGlyph {
                    glyph: infos[k].glyph_id as u16,
                    cell: item.cell,
                    at: at - origin,
                    x_off: poss[k].x_offset as f32 * scale,
                    y_off: poss[k].y_offset as f32 * scale,
                })
            }).collect();
            // How far the word's ink reaches above and below the baseline.
            let (mut up, mut down) = (0.0f32, 0.0f32);
            for &(k, _) in &kept {
                if let Some(ink) = face.glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(infos[k].glyph_id as u16)) {
                    up = up.max((ink.y_max as f32 + poss[k].y_offset as f32) * scale);
                    down = down.max(-(ink.y_min as f32 + poss[k].y_offset as f32) * scale);
                }
            }
            // What stands beside the word decides where its spare room goes:
            // text touching it keeps touching it, and a word that opens the
            // row keeps the margin.
            let inked = |it: &Item| !(it.ch == ' ' && it.marks.is_empty());
            let (west, east) = ((span_x / cell_w).round() as usize, ((span_x + span_w) / cell_w).round() as usize);
            let leads = rtl && phrase.is_empty()
                && self.items.iter().all(|it| !inked(it) || out.column_of(it.cell) < east);
            let held_east = self.items.iter().any(|it| inked(it) && out.column_of(it.cell) == east);
            let held_west = west > 0
                && self.items.iter().any(|it| inked(it) && out.column_of(it.cell) + it.columns == west);
            phrase.push(Word {
                leads: leads || held_east,
                held_west,
                font,
                px,
                glyphs,
                up,
                down,
                natural,
                span_x,
                span_w,
                last,
                gap: if gap_pending { cell_w * WORD_GAP } else { 0.0 },
            });
            gap_pending = false;
            // A phrase is laid out from its right edge; a cursive word in a
            // left-to-right run stands alone.
            if !rtl {
                place_phrase(&mut phrase, out, room, cell_w);
            }
            self.buf = Some(shaped.clear());
        }
        place_phrase(&mut phrase, out, room, cell_w);
    }
}

/// The least blank left between two words of a phrase, in cells. Arabic ink
/// overhangs its advance at both ends, so much less than this reads as one word.
/// Icon fonts keep their symbols in the private use areas. Powerline
/// separators are left out: they join the cells beside them.
fn is_icon(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xE09F | 0xE0D8..=0xF8FF | 0xF0000..=0xFFFFD)
}

/// Width an icon may take when a blank follows it, in cells.
const ICON_ROOM: f32 = 1.5;
/// Icons drawn for one cell are enlarged no further than this.
const ICON_GROWTH: f32 = 2.0;

/// Place an icon by its ink: centred on the text line, scaled to the cell it
/// owns or, with `roomy`, into the blank after it.
fn fit_icon(fonts: &Fonts, font: usize, glyph: u16, px: f32, left: f32, cell_w: f32, roomy: bool) -> Option<PositionedGlyph> {
    let face = &fonts.faces[font];
    let ink = face.hb.glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(glyph))?;
    let em = px / face.upem;
    let (width, height) = ((ink.x_max - ink.x_min) as f32 * em, (ink.y_max - ink.y_min) as f32 * em);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let text = &fonts.faces[fonts.face_id(Slot::Mono, Style::default())];
    let line = (text.ascent - text.descent) * fonts.size_px;
    let room = if roomy { cell_w * ICON_ROOM } else { cell_w };
    let fit = (room / width).min(line * 0.9 / height);
    let k = fit.min(if roomy { ICON_GROWTH } else { 1.0 });
    let middle = (text.ascent + text.descent) * 0.5 * fonts.size_px;
    Some(PositionedGlyph {
        font,
        glyph,
        x: left + ((cell_w - width * k) * 0.5).max(0.0) - ink.x_min as f32 * em * k,
        y: middle - (ink.y_min + ink.y_max) as f32 * 0.5 * em * k,
        px: px * k,
        stretch: 1.0,
        cell: 0,
    })
}

/// Stretch a separator's ink over the row's height, keeping its width.
fn fit_separator(fonts: &Fonts, font: usize, glyph: u16, px: f32, pen: f32) -> Option<PositionedGlyph> {
    let face = &fonts.faces[font];
    let ink = face.hb.glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(glyph))?;
    let em = px / face.upem;
    let height = (ink.y_max - ink.y_min) as f32 * em;
    if height <= 0.0 {
        return None;
    }
    let k = fonts.cell_h / height;
    Some(PositionedGlyph {
        font,
        glyph,
        x: pen,
        y: fonts.baseline - fonts.cell_h - ink.y_min as f32 * em * k,
        px: px * k,
        stretch: (1.0 / k).clamp(0.25, 1.25),
        cell: 0,
    })
}

const WORD_GAP: f32 = 0.6;
/// How far marks may reach into a neighbouring row, as a share of the row.
const MARK_SPILL: f32 = 0.2;

/// One shaped glyph of a [`Word`], positioned relative to the word's origin.
struct WordGlyph {
    glyph: u16,
    cell: usize,
    at: f32,
    x_off: f32,
    y_off: f32,
}

/// A shaped cursive word waiting to be placed with the rest of its phrase.
struct Word {
    font: usize,
    px: f32,
    glyphs: Vec<WordGlyph>,
    /// Reach of the word's ink above and below the baseline at full size.
    up: f32,
    down: f32,
    /// Width of the word's ink at full size.
    natural: f32,
    /// The columns the word owns.
    span_x: f32,
    span_w: f32,
    /// Its last item, for the punctuation that may follow.
    last: usize,
    /// Blank required between this word and the one before it.
    gap: f32,
    /// The word keeps its right edge: it opens the row, or text touches it.
    leads: bool,
    /// Text touches the word's left edge.
    held_west: bool,
}

/// Place a phrase — cursive words separated by single blanks, in logical
/// order — and return its last item with the slack left beyond its final word.
///
/// The grid gives every letter one cell, but ص is far wider than ا. A word of
/// wide letters has more ink than its cells hold while its neighbours have
/// room to spare, so words are not confined to their own cells:
///
/// - Each word starts at the leading edge of its cells unless the word before
///   it has run over, in which case it is pushed along; a word that would
///   overrun the end of the phrase is pulled back into the room before it. The
///   phrase as a whole never leaves the cells it owns.
/// - Only when the whole phrase has more ink than cells is anything made
///   narrower, and then every word by the same factor, shared between size and
///   width. One word at half size beside its neighbours is glaring; a phrase a
///   few percent smaller and a few percent condensed is not noticed.
fn place_phrase(phrase: &mut Vec<Word>, out: &mut RowLayout, room: (f32, f32), cell_w: f32) -> Option<(usize, f32)> {
    let (first, last) = (phrase.first()?, phrase.last()?);
    let (right, left) = (first.span_x + first.span_w, last.span_x);
    let trailing_item = last.last;
    let needed: f32 = phrase.iter().map(|w| w.natural + w.gap).sum();
    let wide = if needed > right - left && needed > 0.0 { (right - left) / needed } else { 1.0 };
    // Stacked marks reach past a row sized for plain letters. They may spill
    // a little into the rows beside it; past that the phrase is drawn
    // smaller, in steps, until its tallest word fits.
    let spill = (room.0 + room.1) * MARK_SPILL;
    let tall = phrase.iter().fold(1.0f32, |fit, w| {
        let up = if w.up > room.0 + spill { (room.0 + spill) / w.up } else { 1.0 };
        let down = if w.down > room.1 + spill { (room.1 + spill) / w.down } else { 1.0 };
        fit.min(up).min(down)
    });
    let tall = if tall < 1.0 { ((tall * 16.0).floor() / 16.0).max(0.5) } else { 1.0 };
    let k = wide.min(tall);
    // Quantised so a row of condensed words shares cached glyphs. Narrowing
    // is shared between size and width, as far as the row's height allows.
    let stretch = if k < tall { ((k.sqrt().max(k / tall)) * 32.0).round() / 32.0 } else { 1.0 };
    let size = k / stretch;

    // Words are usually narrower than their cells. Set close together, with
    // one cell between them, the phrase reads as text; the room left over
    // goes to its two ends, or to one end when the other is held. No word
    // moves more than a cell from the cells it owns, so the pointer and the
    // cursor still land on the word they point at.
    let blank = |w: &Word| if w.gap > 0.0 { cell_w } else { 0.0 };
    let set: f32 = phrase.iter().map(|w| w.natural * k + blank(w)).sum();
    let mut lefts = Vec::with_capacity(phrase.len());
    if set <= right - left {
        let mut pen = if first.leads {
            right
        } else if last.held_west {
            left + set
        } else {
            right - (right - left - set) * 0.5
        };
        let mut shift = 0.0f32;
        for w in phrase.iter() {
            pen -= blank(w) + w.natural * k;
            // A word split by a change of colour stays in one piece.
            if w.gap > 0.0 || lefts.is_empty() {
                let own = w.span_x + (w.span_w - w.natural * k) * 0.5;
                shift = (pen - own).clamp(-cell_w, cell_w) - (pen - own);
            }
            lefts.push(pen + shift);
        }
    } else {
        // Over-full: push along from the leading edge, then pull back from
        // the trailing one.
        let mut limit = f32::INFINITY;
        for w in phrase.iter() {
            let edge = (w.span_x + w.span_w).min(limit - w.gap * k);
            limit = edge - w.natural * k;
            lefts.push(limit);
        }
        let mut bound = left;
        for (w, x) in phrase.iter().zip(lefts.iter_mut()).rev() {
            *x = x.max(bound);
            bound = *x + (w.natural + w.gap) * k;
        }
    }

    let slack = (lefts.last().copied().unwrap_or(left) - left).max(0.0);
    for (w, x) in phrase.drain(..).zip(lefts) {
        for g in w.glyphs {
            out.glyphs.push(PositionedGlyph {
                font: w.font,
                glyph: g.glyph,
                x: x + (g.at + g.x_off) * k,
                y: g.y_off * size,
                px: w.px * size,
                stretch,
                cell: g.cell,
            });
        }
    }
    Some((trailing_item, slack))
}
