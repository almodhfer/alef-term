//! Turning the terminal into pixels: layout lookup, then a stream of
//! rectangles, glyphs and images sent to a [`Target`].

use crate::color::{Palette, Rgb, Theme};
use crate::font::{Fonts, RasterGlyph};
use crate::grid::Attrs;
use crate::layout::{Layouter, RowLayout};
use crate::selection::{Selection, Span};
use crate::term::{CursorShape, Terminal};

/// The search bar's contents.
#[derive(Clone, Copy, Debug)]
pub struct SearchBar<'a> {
    pub query: &'a str,
    pub total: usize,
    /// Zero-based index of the current hit.
    pub current: Option<usize>,
}

/// The theme picker's contents.
#[derive(Clone, Copy, Debug)]
pub struct PickerView<'a> {
    pub query: &'a str,
    /// The themes that match the query, and which of them is highlighted.
    pub themes: &'a [&'static Theme],
    pub selected: usize,
}

/// Everything about a frame that is not terminal content.
#[derive(Clone, Copy, Debug)]
pub struct DrawOpts<'a> {
    pub selection: Option<Selection>,
    /// Unfocused windows show a hollow cursor.
    pub focused: bool,
    /// False during the off half of a blink.
    pub cursor_on: bool,
    /// Overrides the terminal's own cursor shape when the application has not
    /// asked for one; see [`crate::term::CursorStyle`].
    pub cursor_shape: CursorShape,
    /// Bell flash intensity, 0 for none.
    pub bell: f32,
    /// A link under the pointer, underlined so it reads as clickable.
    pub hover_link: Option<Span>,
    /// Search hits, sorted by line then cell, and the spans of the current one.
    pub highlights: &'a [Span],
    pub current: &'a [Span],
    pub search: Option<SearchBar<'a>>,
    pub picker: Option<PickerView<'a>>,
    pub commands: Option<&'a crate::commands::CommandPalette>,
    pub notice: Option<&'a str>,
    /// The window's tabs, when it has more than one.
    pub tabs: Option<TabBar<'a>>,
}

/// The tabs of a window, for the bar across its top.
#[derive(Clone, Copy, Debug)]
pub struct TabBar<'a> {
    pub titles: &'a [String],
    pub active: usize,
}

/// Height of the tab bar: one text row. The grid starts below it.
pub fn tab_bar_height(fonts: &Fonts) -> f32 {
    fonts.cell_h.round()
}

impl Default for DrawOpts<'_> {
    fn default() -> Self {
        DrawOpts {
            selection: None,
            focused: true,
            cursor_on: true,
            cursor_shape: CursorShape::Block,
            bell: 0.0,
            hover_link: None,
            highlights: &[],
            current: &[],
            search: None,
            picker: None,
            commands: None,
            notice: None,
            tabs: None,
        }
    }
}

/// Cells for a string, for text the terminal draws itself (the search bar).
fn cells_of(text: &str) -> crate::grid::Row {
    use unicode_width::UnicodeWidthChar;
    let mut cells: Vec<crate::grid::Cell> = Vec::new();
    for c in text.chars() {
        match c.width().unwrap_or(1) {
            0 => {
                if let Some(last) = cells.last_mut() {
                    last.push_mark(c);
                }
            }
            w => {
                let mut cell = crate::grid::Cell { ch: c, ..Default::default() };
                if w >= 2 {
                    cell.attrs |= Attrs::WIDE_LEADING;
                    cells.push(cell);
                    let mut sp = crate::grid::Cell::default();
                    sp.attrs |= Attrs::WIDE_SPACER;
                    cells.push(sp);
                } else {
                    cells.push(cell);
                }
            }
        }
    }
    crate::grid::Row { cells, wrapped: false }
}

/// Fit terminal UI text without splitting wide glyphs at the edge.
fn fit_cells(text: &str, cols: usize) -> crate::grid::Row {
    let mut row = cells_of(text);
    if row.cells.len() > cols {
        row.cells.truncate(cols.saturating_sub(1));
        if row.cells.last().is_some_and(|c| c.attrs.contains(Attrs::WIDE_LEADING)) {
            row.cells.pop();
        }
        if cols > 0 { row.cells.push(crate::grid::Cell { ch: '…', ..Default::default() }); }
    }
    row
}

/// A cell's underline in its style. `top` is where a single underline sits
/// and `bottom` the bottom of the cell, which the other styles stay above.
fn draw_underline<T: Target>(canvas: &mut T, fonts: &mut Fonts, attrs: Attrs, x: f32, top: f32, bottom: f32, colour: Rgb) {
    let w = fonts.cell_w;
    let dot = (fonts.size_px / 16.0).round().max(1.0);
    if attrs.contains(Attrs::UNDERLINE_CURLY) {
        let wave = fonts.undercurl();
        let oy = (top + 0.5 - wave.height as f32 / 2.0).min(bottom - wave.height as f32);
        canvas.glyph(wave, x.round() as i32, oy.round() as i32, colour);
    } else if attrs.contains(Attrs::UNDERLINE_DOUBLE) {
        let top = top.min(bottom - 3.0);
        canvas.fill_rect(x, top, w, 1.0, colour);
        canvas.fill_rect(x, top + 2.0, w, 1.0, colour);
    } else if attrs.contains(Attrs::UNDERLINE_DOTTED) {
        let mut dx = 0.0;
        while dx < w {
            canvas.fill_rect(x + dx, top, dot.min(w - dx), dot, colour);
            dx += 2.0 * dot;
        }
    } else if attrs.contains(Attrs::UNDERLINE_DASHED) {
        // One dash per cell, centred, so dashes and gaps alternate evenly.
        canvas.fill_rect(x + w * 0.2, top, w * 0.6, 1.0, colour);
    } else {
        canvas.fill_rect(x, top, w, 1.0, colour);
    }
}

/// Somewhere a frame can be drawn: the software [`Canvas`], or the GPU.
///
/// Primitives arrive in back-to-front order and must be composited in that
/// order. Positions are in pixels from the top-left.
pub trait Target {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn clear(&mut self, c: Rgb);
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb);
    /// Draw a rasterised glyph with its top-left at (`ox`, `oy`). `fg` tints
    /// a coverage mask and is ignored for colour glyphs.
    fn glyph(&mut self, g: &RasterGlyph, ox: i32, oy: i32, fg: Rgb);
    /// Draw a straight-alpha RGBA image with its top-left at (`ox`, `oy`).
    fn image(&mut self, img: &crate::sixel::Image, ox: i32, oy: i32);
}

/// A software framebuffer of packed 0x00RRGGBB pixels.
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl<'a> Target for Canvas<'a> {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn clear(&mut self, c: Rgb) {
        self.buf.fill(c.pack());
    }

    fn glyph(&mut self, g: &RasterGlyph, ox: i32, oy: i32, fg: Rgb) {
        if g.color {
            self.blit_color(&g.data, g.width as usize, g.height as usize, ox, oy);
        } else {
            self.blit_mask(&g.data, g.width as usize, g.height as usize, ox, oy, fg);
        }
    }

    fn image(&mut self, img: &crate::sixel::Image, ox: i32, oy: i32) {
        self.blit_rgba(&img.rgba, img.width as usize, img.height as usize, ox, oy);
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
        let x0 = x.round().max(0.0) as usize;
        let y0 = y.round().max(0.0) as usize;
        let x1 = ((x + w).round().max(0.0) as usize).min(self.width);
        let y1 = ((y + h).round().max(0.0) as usize).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let packed = c.pack();
        for row in y0..y1 {
            let base = row * self.width;
            self.buf[base + x0..base + x1].fill(packed);
        }
    }
}

impl<'a> Canvas<'a> {
    /// Clip a `gw`×`gh` bitmap placed at (`ox`, `oy`) to the canvas.
    ///
    /// Returns the source column range, the destination x range, and the
    /// source/destination row pairs, so the blitters below run their inner
    /// loops with no per-pixel bounds checks. Done once per glyph rather than
    /// once per pixel; the blitters are the hottest loop of a static frame.
    #[inline]
    fn clip(
        &self,
        gw: usize,
        gh: usize,
        ox: i32,
        oy: i32,
    ) -> Option<(std::ops::Range<usize>, usize, std::ops::Range<usize>)> {
        let x0 = ox.max(0);
        let x1 = (ox + gw as i32).min(self.width as i32);
        let y0 = oy.max(0);
        let y1 = (oy + gh as i32).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let sx = (x0 - ox) as usize..(x1 - ox) as usize;
        Some((sx, x0 as usize, y0 as usize..y1 as usize))
    }

    /// Blend an 8-bit coverage mask in `fg` over whatever is already there.
    fn blit_mask(&mut self, data: &[u8], gw: usize, gh: usize, ox: i32, oy: i32, fg: Rgb) {
        let Some((sx, dx, ys)) = self.clip(gw, gh, ox, oy) else { return };
        let (fr, fg_, fb) = (fg.0 as u32, fg.1 as u32, fg.2 as u32);
        let solid = (fr << 16) | (fg_ << 8) | fb;
        for y in ys {
            let row = (y as i32 - oy) as usize;
            let src = &data[row * gw + sx.start..row * gw + sx.end];
            let base = y * self.width + dx;
            let dst = &mut self.buf[base..base + src.len()];
            for (d, &a) in dst.iter_mut().zip(src) {
                let a = a as u32;
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    *d = solid;
                    continue;
                }
                let inv = 255 - a;
                let dr = (*d >> 16) & 0xff;
                let dg = (*d >> 8) & 0xff;
                let db = *d & 0xff;
                let r = (fr * a + dr * inv + 127) / 255;
                let g = (fg_ * a + dg * inv + 127) / 255;
                let b = (fb * a + db * inv + 127) / 255;
                *d = (r << 16) | (g << 8) | b;
            }
        }
    }

    /// Blend straight (non-premultiplied) RGBA image data.
    pub fn blit_rgba(&mut self, data: &[u8], gw: usize, gh: usize, ox: i32, oy: i32) {
        let Some((sx, dx, ys)) = self.clip(gw, gh, ox, oy) else { return };
        for y in ys {
            let row = (y as i32 - oy) as usize;
            let src = &data[(row * gw + sx.start) * 4..(row * gw + sx.end) * 4];
            let base = y * self.width + dx;
            let dst = &mut self.buf[base..base + sx.len()];
            for (d, px) in dst.iter_mut().zip(src.chunks_exact(4)) {
                let a = px[3] as u32;
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    *d = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
                    continue;
                }
                let inv = 255 - a;
                let dr = (*d >> 16) & 0xff;
                let dg = (*d >> 8) & 0xff;
                let db = *d & 0xff;
                let r = (px[0] as u32 * a + dr * inv + 127) / 255;
                let g = (px[1] as u32 * a + dg * inv + 127) / 255;
                let b = (px[2] as u32 * a + db * inv + 127) / 255;
                *d = (r << 16) | (g << 8) | b;
            }
        }
    }

    /// Blend premultiplied-ish BGRA colour bitmap data (colour emoji).
    fn blit_color(&mut self, data: &[u8], gw: usize, gh: usize, ox: i32, oy: i32) {
        let Some((sx, dx, ys)) = self.clip(gw, gh, ox, oy) else { return };
        for y in ys {
            let row = (y as i32 - oy) as usize;
            let src = &data[(row * gw + sx.start) * 4..(row * gw + sx.end) * 4];
            let base = y * self.width + dx;
            let dst = &mut self.buf[base..base + sx.len()];
            for (d, px) in dst.iter_mut().zip(src.chunks_exact(4)) {
                let a = px[3] as u32;
                if a == 0 {
                    continue;
                }
                let (sr, sg, sb) = (px[0] as u32, px[1] as u32, px[2] as u32);
                let inv = 255 - a;
                let dr = (*d >> 16) & 0xff;
                let dg = (*d >> 8) & 0xff;
                let db = *d & 0xff;
                let r = (sr * 255 + dr * inv + 127) / 255;
                let g = (sg * 255 + dg * inv + 127) / 255;
                let b = (sb * 255 + db * inv + 127) / 255;
                *d = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
            }
        }
    }
}

/// Hash of everything about a row that affects its layout — deliberately not
/// its colours, which are looked up per cell at blit time.
fn layout_key(
    row: &crate::grid::Row,
    cols: usize,
    pre: &str,
    post: &str,
    mode: crate::term::BidiMode,
    bidi_on: bool,
    generation: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = crate::hash::FoldHasher::default();
    cols.hash(&mut h);
    pre.hash(&mut h);
    post.hash(&mut h);
    (mode as u8).hash(&mut h);
    bidi_on.hash(&mut h);
    generation.hash(&mut h);
    for cell in row.cells.iter().take(cols) {
        cell.ch.hash(&mut h);
        cell.marks().hash(&mut h);
        // Only the attributes that pick a font, or a cell's width, matter.
        (cell.attrs & (Attrs::BOLD | Attrs::ITALIC | Attrs::WIDE_LEADING | Attrs::WIDE_SPACER))
            .bits()
            .hash(&mut h);
    }
    h.finish()
}

pub struct Renderer {
    pub layouter: Layouter,
    /// Readability policy, 1 disables adjustment. The application configures it.
    pub minimum_contrast: f32,
    contrast_cache: crate::hash::FastMap<(u32, u32, u32), Rgb>,
    /// Cache key of each visible row's layout from the last frame. Mouse
    /// hit-testing looks the layout up through these.
    layouts: Vec<u64>,
    /// Inset of the grid from the window edge, in pixels. Glyph ink routinely
    /// overhangs its advance width, so a run flush against the right edge would
    /// otherwise be shaved off — very visible on right-aligned RTL rows.
    pub pad_x: f32,
    pub pad_y: f32,
    /// Shaped layouts keyed by content. Shaping a row is by far the most
    /// expensive part of a frame, and between frames most rows do not change.
    cache: std::collections::HashMap<u64, RowLayout>,
    /// Keys touched this frame, used to evict what has gone stale.
    live: Vec<u64>,
    /// Sub-line scroll offset, 0..1 of a cell. Non-zero while a smooth scroll
    /// is in flight, which is what lets content move by fractions of a row.
    pub scroll_frac: f32,
    /// Opacity of the scrollbar, faded out when idle.
    pub scrollbar_alpha: f32,
}

impl Renderer {
    /// Font families can change on settings reload without changing the size.
    pub fn invalidate_layouts(&mut self) {
        self.layouts.clear();
        self.cache.clear();
        self.live.clear();
    }

    /// Layout of a visible row from the last frame, for mouse hit-testing.
    pub fn row_layout(&self, row: usize) -> Option<&RowLayout> {
        self.layouts.get(row).and_then(|k| self.cache.get(k))
    }

    pub fn new() -> Self {
        Renderer {
            layouter: Layouter::new(),
            minimum_contrast: 1.0,
            contrast_cache: Default::default(),
            layouts: Vec::new(),
            pad_x: 12.0,
            pad_y: 10.0,
            cache: std::collections::HashMap::new(),
            live: Vec::new(),
            scroll_frac: 0.0,
            scrollbar_alpha: 0.0,
        }
    }

    pub fn draw<T: Target>(
        &mut self,
        canvas: &mut T,
        term: &Terminal,
        fonts: &mut Fonts,
        opts: &DrawOpts,
    ) {
        let palette: &Palette = &term.palette;
        // Keep repeated colours cheap without growing indefinitely on truecolor artwork.
        if self.contrast_cache.len() > 4096 { self.contrast_cache.clear(); }
        let selection = opts.selection;
        let reverse = term.modes.contains(crate::term::Modes::REVERSE_VIDEO);
        let def_bg = if reverse { palette.fg } else { palette.bg };
        canvas.clear(def_bg);

        let cell_w = fonts.cell_w;
        let cell_h = fonts.cell_h;
        let baseline = fonts.baseline;
        let cols = term.grid.cols;
        let rows = term.grid.rows;
        let bar_h = if opts.tabs.is_some() { tab_bar_height(fonts) } else { 0.0 };
        let grid_top = self.pad_y + bar_h;

        let mut keys: Vec<u64> = Vec::with_capacity(rows + 1);
        self.live.clear();
        // While smooth-scrolling, one extra row is visible at each edge.
        let frac = self.scroll_frac.clamp(0.0, 1.0);
        let first: isize = if frac > 0.0 { -1 } else { 0 };
        let bidi_on = term.modes.contains(crate::term::Modes::BIDI);

        let top = term.grid.viewport_top();
        let mut highlights = vec![None; cols];
        let mut colours = Vec::with_capacity(cols);
        for ri in first..rows as isize {
            let abs = top + ri;
            let Some(row) = term.grid.line(abs) else {
                if ri >= 0 {
                    keys.push(0);
                }
                continue;
            };

            // Joining context across a soft wrap: a word broken by the right
            // margin has not ended, so its letters must still take their joined
            // forms on both sides of the break.
            let pre = match term.grid.line(abs - 1) {
                Some(prev) if prev.wrapped => row_tail(prev, cols),
                _ => String::new(),
            };
            let post = if row.wrapped {
                match term.grid.line(abs + 1) {
                    Some(next) => row_head(next, cols),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            let key =
                layout_key(row, cols, &pre, &post, term.bidi_mode, bidi_on, fonts.generation());
            self.live.push(key);
            if !self.cache.contains_key(&key) {
                let fresh = self.layouter.layout_row_ctx(
                    row,
                    cols,
                    fonts,
                    term.bidi_mode,
                    bidi_on,
                    crate::layout::RowContext { pre: &pre, post: &post },
                );
                self.cache.insert(key, fresh);
            }
            let layout = self.cache.get(&key).expect("just inserted");
            let y = grid_top + (ri as f32 + frac) * cell_h;

            // Resolve each logical cell once so text, borders and decorations
            // share the same inverse, dim and highlight colours. Selection
            // sits above search hits; the current hit sits above other hits.
            highlights.fill(None);
            if abs >= 0 {
                for (spans, bg) in [(opts.highlights, palette.search_bg), (opts.current, palette.search_current_bg)] {
                    let lo = spans.partition_point(|s| s.line < abs as usize);
                    for s in &spans[lo..] {
                        if s.line != abs as usize { break; }
                        highlights[s.start.min(cols)..s.end.min(cols)].fill(Some(bg));
                    }
                }
            }
            colours.clear();
            colours.extend(row.cells.iter().take(cols).enumerate().map(|(cell_idx, cell)| {
                let mut fg = palette.resolve(cell.fg, true);
                let mut bg = palette.resolve(cell.bg, false);
                if cell.attrs.contains(Attrs::INVERSE) ^ reverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.attrs.contains(Attrs::DIM) {
                    fg = fg.dimmed(bg);
                }
                let selected = ri >= 0
                    && abs >= 0
                    && selection.map_or(false, |s| s.contains(abs as usize, cell_idx));
                if selected {
                    bg = palette.selection_bg;
                    fg = palette.selection_fg.unwrap_or_else(|| palette.highlight_fg(fg, bg));
                } else if let Some(highlight) = highlights[cell_idx] {
                    bg = highlight;
                    fg = palette.highlight_fg(fg, bg);
                } else if self.minimum_contrast > 1.0
                    && !cell.attrs.contains(Attrs::HIDDEN)
                    && !matches!(cell.ch, '\u{2580}'..='\u{259f}' | '\u{e0b0}'..='\u{e0d7}') {
                    // Faint remains less prominent. Preserve block graphics and
                    // Powerline separators, whose ink represents a surface.
                    let minimum = if cell.attrs.contains(Attrs::DIM) {
                        (self.minimum_contrast / 1.5).max(1.0)
                    } else { self.minimum_contrast };
                    let key = (fg.pack(), bg.pack(), minimum.to_bits());
                    fg = *self.contrast_cache.entry(key).or_insert_with(|| fg.ensure_contrast(bg, minimum));
                }
                (fg, bg)
            }));

            // --- backgrounds, in visual column order ---
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(&(_, bg)) = colours.get(cell_idx) else { continue };
                if bg != def_bg {
                    canvas.fill_rect(self.pad_x + col as f32 * cell_w, y, cell_w, cell_h, bg);
                }
            }

            // --- procedural box drawing, per visual column ---
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(cell) = row.cells.get(cell_idx) else { continue };
                if !crate::boxdraw::is_procedural(cell.ch) || cell.attrs.contains(Attrs::HIDDEN) {
                    continue;
                }
                let fg = colours[cell_idx].0;
                crate::boxdraw::draw(
                    canvas,
                    cell.ch,
                    self.pad_x + col as f32 * cell_w,
                    y,
                    cell_w,
                    cell_h,
                    fg,
                );
            }

            // --- glyphs ---
            for g in &layout.glyphs {
                // Colour and attributes come from the cell, not the glyph, so
                // the layout above can be cached across a colour change.
                let Some(cell) = row.cells.get(g.cell) else { continue };
                if cell.attrs.contains(Attrs::HIDDEN) {
                    continue;
                }
                let fg = colours[g.cell].0;

                let gx = self.pad_x + g.x;
                let gy = y + baseline - g.y;
                let Some(raster) = fonts.rasterize_condensed(g.font, g.glyph, g.px, gx, g.stretch) else { continue };
                let ox = gx.floor() as i32 + raster.left;
                let oy = gy.floor() as i32 - raster.top;
                canvas.glyph(raster, ox, oy, fg);
            }

            // --- underline / strikeout, drawn per visual column so they run
            // continuously under reordered text ---
            let hover = opts.hover_link.filter(|h| abs >= 0 && h.line == abs as usize);
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(cell) = row.cells.get(cell_idx) else { continue };
                if cell.attrs.contains(Attrs::HIDDEN) { continue; }
                if hover.is_some_and(|h| cell_idx >= h.start && cell_idx < h.end) {
                    let x = self.pad_x + col as f32 * cell_w;
                    canvas.fill_rect(x, y + baseline + 2.0, cell_w, 1.0, palette.cursor);
                }
                if cell.attrs.intersects(Attrs::UNDERLINE | Attrs::STRIKEOUT) {
                    let fg = colours[cell_idx].0;
                    let x = self.pad_x + col as f32 * cell_w;
                    if cell.attrs.contains(Attrs::UNDERLINE) {
                        let colour = cell.underline_color().map_or(fg, |c| palette.resolve(c, true));
                        draw_underline(canvas, fonts, cell.attrs, x, y + baseline + 2.0, y + cell_h, colour);
                    }
                    if cell.attrs.contains(Attrs::STRIKEOUT) {
                        canvas.fill_rect(x, y + baseline - cell_h * 0.25, cell_w, 1.0, fg);
                    }
                }
            }

            if ri >= 0 {
                keys.push(key);
            }
        }

        self.layouts = keys;
        // Evict layouts no longer on screen, so scrollback does not accumulate.
        if self.cache.len() > self.layouts.len() * 4 + 256 {
            let live: std::collections::HashSet<u64> = self.layouts.iter().copied().collect();
            self.cache.retain(|k, _| live.contains(k));
        }

        // --- images, drawn under the cursor but over the text background ---
        for placed in &term.images {
            // The image is pinned to an absolute line; work out where that
            // lands in the viewport, if anywhere.
            let top_line = term.grid.scrolled.saturating_sub(term.grid.view_offset as u64);
            let Some(rel) = placed.line.checked_sub(top_line) else { continue };
            if rel >= rows as u64 + 64 {
                continue;
            }
            let y = grid_top + (rel as f32 + frac) * cell_h;
            let x = self.pad_x + placed.col as f32 * cell_w;
            if y > canvas.height() as f32 {
                continue;
            }
            canvas.image(&placed.image, x.round() as i32, y.round() as i32);
        }

        // --- scrollbar, shown while scrolled back and faded out when idle ---
        if self.scrollbar_alpha > 0.01 && !term.grid.in_alt() {
            let total = term.grid.total_lines().max(1);
            if total > rows {
                let track_h = rows as f32 * cell_h;
                let thumb_h = (rows as f32 / total as f32 * track_h).max(cell_h);
                // view_offset counts lines back from the live screen, so a zero
                // offset puts the thumb at the bottom.
                let scrolled = term.grid.view_offset as f32;
                let max_scroll = (total - rows) as f32;
                let t = 1.0 - (scrolled / max_scroll.max(1.0)).clamp(0.0, 1.0);
                let y = grid_top + t * (track_h - thumb_h);
                let w = 4.0;
                let x = canvas.width() as f32 - w - 2.0;
                let a = self.scrollbar_alpha.clamp(0.0, 1.0);
                let fg = palette.selection_bg;
                let blend = |c: u8, d: u8| ((c as f32 * a) + (d as f32 * (1.0 - a))) as u8;
                let col = Rgb(
                    blend(fg.0, def_bg.0),
                    blend(fg.1, def_bg.1),
                    blend(fg.2, def_bg.2),
                );
                canvas.fill_rect(x, y, w, thumb_h, col);
            }
        }

        // --- tab bar, over the edge of a row scrolling out at the top ---
        if let Some(tabs) = opts.tabs {
            self.draw_tab_bar(canvas, fonts, term, &tabs, bar_h);
        }

        // --- search bar, top right, drawn over everything ---
        if let Some(bar) = opts.search {
            let pad = 6.0;
            let count = match (bar.current, bar.total) {
                (_, 0) if bar.query.is_empty() => String::new(),
                (_, 0) => "no matches".to_string(),
                (Some(i), n) => format!("{}/{n}", i + 1),
                (None, n) => format!("{n}"),
            };
            let query_cells = cells_of(bar.query);
            let count_cells = cells_of(&count);
            let qw = query_cells.cells.len().max(12) as f32 * cell_w;
            let cw_ = count_cells.cells.len() as f32 * cell_w;
            let w = pad * 2.0 + qw + cell_w + cw_ + cell_w;
            let h = cell_h + pad * 2.0;
            let x0 = canvas.width() as f32 - w - 8.0;
            let y0 = bar_h + 6.0;
            canvas.fill_rect(x0 - 1.0, y0 - 1.0, w + 2.0, h + 2.0, palette.cursor);
            canvas.fill_rect(x0, y0, w, h, palette.bg);
            let ty = y0 + pad;
            let rtl = self.draw_text(canvas, fonts, &query_cells, x0 + pad, ty, palette.fg, palette.bg, term);
            // The caret sits where the next character goes.
            let n = query_cells.cells.len() as f32;
            let caret_x = if rtl { x0 + pad + qw - n * cell_w - 1.0 } else { x0 + pad + n * cell_w };
            canvas.fill_rect(caret_x, ty, 2.0, cell_h, palette.cursor);
            self.draw_text(canvas, fonts, &count_cells, x0 + w - pad - cw_, ty, palette.fg.dimmed(palette.bg), palette.bg, term);
        }

        // --- theme picker, centred over everything ---
        if let Some(p) = &opts.picker {
            self.draw_picker(canvas, fonts, term, p);
        }
        if let Some(commands) = opts.commands {
            self.draw_commands(canvas, fonts, term, commands);
        }
        if let Some(message) = opts.notice {
            let cols = ((canvas.width() as f32 - 2.0 * self.pad_x) / cell_w).max(0.0) as usize;
            let row = fit_cells(message, cols.saturating_sub(2));
            let y = (canvas.height() as f32 - self.pad_y - cell_h).max(0.0);
            canvas.fill_rect(self.pad_x, y, (row.cells.len() + 2) as f32 * cell_w, cell_h, palette.selection_bg);
            self.draw_text(canvas, fonts, &row, self.pad_x + cell_w, y,
                palette.highlight_fg(palette.fg, palette.selection_bg), palette.selection_bg, term);
        }

        // --- bell: a brief flash of the window edge ---
        if opts.bell > 0.01 {
            let a = opts.bell.clamp(0.0, 1.0);
            let blend = |c: u8, d: u8| ((c as f32 * a) + (d as f32 * (1.0 - a))) as u8;
            let fg = palette.cursor;
            let col = Rgb(blend(fg.0, def_bg.0), blend(fg.1, def_bg.1), blend(fg.2, def_bg.2));
            let (w, h) = (canvas.width() as f32, canvas.height() as f32);
            let t = 3.0;
            canvas.fill_rect(0.0, 0.0, w, t, col);
            canvas.fill_rect(0.0, h - t, w, t, col);
            canvas.fill_rect(0.0, 0.0, t, h, col);
            canvas.fill_rect(w - t, 0.0, t, h, col);
        }

        // --- cursor, mapped through the row's logical->visual column map ---
        let cursor_shown = term.modes.contains(crate::term::Modes::CURSOR_VISIBLE)
            && term.grid.view_offset == 0
            && (opts.cursor_on || !opts.focused)
            && opts.picker.is_none() && opts.commands.is_none();
        if cursor_shown {
            let cur = term.grid.screen.cursor;
            if cur.row < rows {
                let Some(layout) = self.layouts.get(cur.row).and_then(|k| self.cache.get(k))
                else {
                    return;
                };
                let col = layout.column_of(cur.col.min(cols.saturating_sub(1)));
                let cursor_rtl = layout.is_rtl(cur.col);
                let x = self.pad_x + col as f32 * cell_w;
                let y = grid_top + (cur.row as f32 + frac) * cell_h;
                let shape = match term.cursor_style.blink {
                    // An application that set a style gets its shape.
                    Some(_) => term.cursor_style.shape,
                    None => opts.cursor_shape,
                };
                if !opts.focused {
                    // Hollow box when unfocused, whatever the shape.
                    canvas.fill_rect(x, y, cell_w, 1.0, palette.cursor);
                    canvas.fill_rect(x, y + cell_h - 1.0, cell_w, 1.0, palette.cursor);
                    canvas.fill_rect(x, y, 1.0, cell_h, palette.cursor);
                    canvas.fill_rect(x + cell_w - 1.0, y, 1.0, cell_h, palette.cursor);
                } else if shape == CursorShape::Underline {
                    canvas.fill_rect(x, y + cell_h - 2.0, cell_w, 2.0, palette.cursor);
                } else if shape == CursorShape::Bar {
                    // On the leading edge: the side the next character lands on.
                    let bx = if cursor_rtl { x + cell_w - 2.0 } else { x };
                    canvas.fill_rect(bx, y, 2.0, cell_h, palette.cursor);
                } else {
                    canvas.fill_rect(x, y, cell_w, cell_h, palette.cursor);
                    // Re-draw the glyph(s) of that cell in the background
                    // colour so the character stays legible under the block.
                    let row = term.grid.viewport_row(cur.row);
                    let glyphs: Vec<_> =
                        layout.glyphs.iter().filter(|g| g.cell == cur.col).copied().collect();
                    for g in glyphs {
                        let gy = y + baseline - g.y;
                        let gx = self.pad_x + g.x;
                        if let Some(raster) = fonts.rasterize_condensed(g.font, g.glyph, g.px, gx, g.stretch) {
                            let ox = gx.floor() as i32 + raster.left;
                            let oy = gy.floor() as i32 - raster.top;
                            if !raster.color {
                                canvas.glyph(raster, ox, oy, palette.highlight_fg(def_bg, palette.cursor));
                            }
                        }
                    }
                    let _ = row;
                }
                // A thin marker on the leading edge shows the writing direction
                // the cursor will advance in — invaluable in mixed text.
                if layout.rtl {
                    let edge = if cursor_rtl { x } else { x + cell_w - 2.0 };
                    canvas.fill_rect(edge, y, 2.0, cell_h * 0.35, palette.selection_bg);
                }
            }
        }
    }
}

/// The picker's key hint.
const PICKER_HINT: &str = "↑↓ choose · Enter apply · Esc cancel";

impl Renderer {
    /// One tab per session, sharing the width. The active tab takes the
    /// terminal's background and opens onto it, so it reads as the page in
    /// front; the others sit on a slightly tinted strip.
    fn draw_tab_bar<T: Target>(&mut self, canvas: &mut T, fonts: &mut Fonts, term: &Terminal, bar: &TabBar, height: f32) {
        let palette = &term.palette;
        let (cell_w, cell_h) = (fonts.cell_w, fonts.cell_h);
        let width = canvas.width() as f32;
        let strip = crate::color::mix(palette.bg, palette.fg, 24);
        let rule = crate::color::mix(palette.bg, palette.fg, 64);
        canvas.fill_rect(0.0, 0.0, width, height, strip);
        let n = bar.titles.len().max(1);
        let span = |i: usize| {
            let x = (i as f32 * width / n as f32).round();
            (x, ((i + 1) as f32 * width / n as f32).round() - x)
        };
        for (i, title) in bar.titles.iter().enumerate() {
            let (x, w) = span(i);
            let active = i == bar.active;
            let (bg, fg) = if active { (palette.bg, palette.fg) } else { (strip, palette.fg.dimmed(strip)) };
            if active {
                canvas.fill_rect(x, 0.0, w, height, bg);
                canvas.fill_rect(x, 0.0, w, 2.0, palette.cursor);
            } else if i + 1 < n && i + 1 != bar.active {
                canvas.fill_rect(x + w - 1.0, height * 0.25, 1.0, height * 0.5, rule);
            }
            let row = fit_cells(title, ((w / cell_w) as usize).saturating_sub(2));
            let tx = x + ((w - row.cells.len() as f32 * cell_w) / 2.0).max(cell_w);
            self.draw_text(canvas, fonts, &row, tx, ((height - cell_h) / 2.0).round(), fg, bg, term);
        }
        let (ax, aw) = span(bar.active.min(n - 1));
        canvas.fill_rect(0.0, height - 1.0, ax, 1.0, rule);
        canvas.fill_rect(ax + aw, height - 1.0, width - ax - aw, 1.0, rule);
    }

    fn draw_commands<T: Target>(&mut self, canvas: &mut T, fonts: &mut Fonts, term: &Terminal,
        p: &crate::commands::CommandPalette) {
        let palette = &term.palette;
        let (cw, ch) = (fonts.cell_w, fonts.cell_h);
        if (canvas.width() as f32) < cw * 6.0 || (canvas.height() as f32) < ch * 5.0 {
            let cols = (canvas.width() as f32 / cw).floor().max(0.0) as usize;
            canvas.fill_rect(0.0, 0.0, canvas.width() as f32, ch, palette.bg);
            self.draw_text(canvas, fonts, &fit_cells(&format!("> {}", p.query), cols),
                0.0, 0.0, palette.fg, palette.bg, term);
            return;
        }
        let cols = ((canvas.width() as f32 - 4.0 * cw) / cw).floor().max(4.0) as usize;
        let cols = cols.min(62);
        let matches = p.matches();
        let room = ((canvas.height() as f32 / ch).floor() as usize).saturating_sub(7).max(1).min(10);
        let shown = matches.len().max(1).min(room);
        let w = (cols + 2) as f32 * cw;
        let h = (shown + 4) as f32 * ch;
        let x = ((canvas.width() as f32 - w) * 0.5).max(0.0).round();
        let y = ((canvas.height() as f32 - h) * 0.25).max(0.0).round();
        canvas.fill_rect(x - 1.0, y - 1.0, w + 2.0, h + 2.0, palette.cursor);
        canvas.fill_rect(x, y, w, h, palette.bg);
        let header = fit_cells(&format!("> {}", p.query), cols.saturating_sub(1));
        self.draw_text(canvas, fonts, &header, x + cw, y + ch * 0.5, palette.fg, palette.bg, term);
        canvas.fill_rect(x + (header.cells.len() + 1) as f32 * cw, y + ch * 0.5, 2.0, ch, palette.cursor);
        let first = p.selected.saturating_sub(shown / 2).min(matches.len().saturating_sub(shown));
        for (row, &id) in matches.iter().skip(first).take(shown).enumerate() {
            let selected = row + first == p.selected;
            let bg = if selected { palette.selection_bg } else { palette.bg };
            let fg = palette.highlight_fg(palette.fg, bg);
            let yy = y + (row as f32 + 2.0) * ch;
            canvas.fill_rect(x + cw * 0.5, yy, w - cw, ch, bg);
            let (_, label, _) = crate::commands::ACTIONS[id];
            let hint = crate::commands::hint(id);
            let hint_cols = if cols >= 36 { hint.chars().count() } else { 0 };
            let label = fit_cells(label, cols.saturating_sub(hint_cols + 2));
            self.draw_text(canvas, fonts, &label, x + cw, yy, fg, bg, term);
            if hint_cols > 0 {
                self.draw_text(canvas, fonts, &cells_of(hint), x + w - (hint_cols + 1) as f32 * cw,
                    yy, fg, bg, term);
            }
        }
        if matches.is_empty() {
            self.draw_text(canvas, fonts, &fit_cells("No matching commands", cols), x + cw,
                y + 2.0 * ch, palette.fg, palette.bg, term);
        }
        self.draw_text(canvas, fonts, &fit_cells("↑↓ choose · Enter run · Esc close", cols), x + cw,
            y + h - ch * 1.5, palette.fg.dimmed(palette.bg), palette.bg, term);
    }

    /// The theme picker: a query line, the matching themes each painted in
    /// its own colours with a swatch of its accents, and a key hint.
    fn draw_picker<T: Target>(&mut self, canvas: &mut T, fonts: &mut Fonts, term: &Terminal, p: &PickerView) {
        const SWATCHES: usize = 6;
        let palette = &term.palette;
        let (cell_w, cell_h) = (fonts.cell_w, fonts.cell_h);
        let pad = 8.0;
        let row_h = (cell_h + 6.0).round();
        let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
        // Secondary text, halfway to the background: some themes' bright
        // black is too faint to read as text.
        let dim = palette.fg.dimmed(palette.bg);

        let title_cols = p.themes.iter().map(|t| t.title.chars().count()).max().unwrap_or(0).max(16);
        let inner_cols = (1 + title_cols + 2 + SWATCHES + 1).max(PICKER_HINT.chars().count() + 1);
        let w = (inner_cols as f32 * cell_w + pad * 2.0).min(cw - 16.0).max(cell_w * 4.0);
        // As many rows as the window has room for beside the query and hint.
        let chrome = pad * 4.0 + cell_h * 2.0;
        let room = ((ch - 32.0 - chrome) / row_h).floor().max(1.0) as usize;
        let shown = p.themes.len().min(room);
        let h = chrome + shown.max(1) as f32 * row_h;
        let x0 = ((cw - w) / 2.0).max(0.0).round();
        let y0 = ((ch - h) / 2.0).max(0.0).round();
        canvas.fill_rect(x0 - 1.0, y0 - 1.0, w + 2.0, h + 2.0, palette.cursor);
        canvas.fill_rect(x0, y0, w, h, palette.bg);

        // The query, after a label, with the caret where typing goes.
        let mut y = y0 + pad;
        let label = cells_of("Theme: ");
        self.draw_text(canvas, fonts, &label, x0 + pad, y, dim, palette.bg, term);
        let qx = x0 + pad + label.cells.len() as f32 * cell_w;
        let query = cells_of(p.query);
        self.draw_text(canvas, fonts, &query, qx, y, palette.fg, palette.bg, term);
        canvas.fill_rect(qx + query.cells.len() as f32 * cell_w, y, 2.0, cell_h, palette.cursor);
        y += cell_h + pad;

        let (rx, rw) = (x0 + pad, w - pad * 2.0);
        let text_dy = ((row_h - cell_h) / 2.0).floor();
        if p.themes.is_empty() {
            let none = cells_of("no theme matches");
            self.draw_text(canvas, fonts, &none, rx + cell_w, y + text_dy, dim, palette.bg, term);
        }
        // A window onto the list that keeps the highlight in view.
        let first = p.selected.saturating_sub(shown / 2).min(p.themes.len() - shown);
        for (i, t) in p.themes.iter().enumerate().skip(first).take(shown) {
            let ry = y + (i - first) as f32 * row_h;
            canvas.fill_rect(rx, ry + 1.0, rw, row_h - 2.0, t.bg);
            let name = cells_of(t.title);
            self.draw_text(canvas, fonts, &name, rx + cell_w, ry + text_dy, t.fg, t.bg, term);
            // Red through cyan: what the theme does to `ls` and a prompt.
            let sh = (cell_h * 0.5).round();
            let sx = rx + rw - (SWATCHES + 1) as f32 * cell_w;
            for (k, c) in t.ansi[1..=SWATCHES].iter().enumerate() {
                canvas.fill_rect(sx + k as f32 * cell_w + 1.0, ry + (row_h - sh) / 2.0, cell_w - 2.0, sh, *c);
            }
        }
        if p.selected >= first && p.selected < first + shown {
            // A ring round the highlighted row, in the colours on show.
            let ry = y + (p.selected - first) as f32 * row_h;
            let c = palette.cursor;
            canvas.fill_rect(rx - 2.0, ry - 1.0, rw + 4.0, 2.0, c);
            canvas.fill_rect(rx - 2.0, ry + row_h - 1.0, rw + 4.0, 2.0, c);
            canvas.fill_rect(rx - 2.0, ry - 1.0, 2.0, row_h + 2.0, c);
            canvas.fill_rect(rx + rw, ry - 1.0, 2.0, row_h + 2.0, c);
        }

        let hy = y + shown.max(1) as f32 * row_h + pad;
        let hint = cells_of(PICKER_HINT);
        self.draw_text(canvas, fonts, &hint, x0 + pad, hy, dim, palette.bg, term);
    }

    /// Draw one row of the terminal's own text at (`x`, `y`), the row's
    /// top-left. Returns whether it laid out right-to-left, which is where
    /// its caret belongs. `bg` is where the row is drawn, for colour glyphs.
    #[allow(clippy::too_many_arguments)]
    fn draw_text<T: Target>(
        &mut self,
        canvas: &mut T,
        fonts: &mut Fonts,
        row: &crate::grid::Row,
        x: f32,
        y: f32,
        fg: Rgb,
        _bg: Rgb,
        term: &Terminal,
    ) -> bool {
        let _ = term;
        let cols = row.cells.len();
        if cols == 0 {
            return false;
        }
        let layout = self.layouter.layout_row_ctx(
            row,
            cols,
            fonts,
            crate::term::BidiMode::Auto,
            true,
            crate::layout::RowContext { pre: "", post: "" },
        );
        let baseline = fonts.baseline;
        for g in &layout.glyphs {
            let gx = x + g.x;
            let gy = y + baseline - g.y;
            let Some(raster) = fonts.rasterize_condensed(g.font, g.glyph, g.px, gx, g.stretch) else { continue };
            let ox = gx.floor() as i32 + raster.left;
            let oy = gy.floor() as i32 - raster.top;
            canvas.glyph(raster, ox, oy, fg);
        }
        layout.rtl
    }
}

/// Characters to hand the shaper as context. Two is enough: a cursive form
/// depends only on its immediate neighbours.
const CONTEXT_CHARS: usize = 2;

/// The last few characters of a row, for use as shaping pre-context.
fn row_tail(row: &crate::grid::Row, cols: usize) -> String {
    let end = row.cells.len().min(cols);
    let mut out: Vec<char> = Vec::new();
    for cell in row.cells[..end].iter().rev() {
        if cell.attrs.contains(Attrs::WIDE_SPACER) {
            continue;
        }
        out.push(cell.ch);
        if out.len() == CONTEXT_CHARS {
            break;
        }
    }
    out.reverse();
    out.into_iter().collect()
}

/// The first few characters of a row, for use as shaping post-context.
fn row_head(row: &crate::grid::Row, cols: usize) -> String {
    row.cells
        .iter()
        .take(cols)
        .filter(|c| !c.attrs.contains(Attrs::WIDE_SPACER))
        .take(CONTEXT_CHARS)
        .map(|c| c.ch)
        .collect()
}
