//! Software rasterisation of the terminal into a softbuffer framebuffer.

use crate::color::{Color, Palette, Rgb};
use crate::font::Fonts;
use crate::grid::Attrs;
use crate::layout::{Layouter, RowLayout};
use crate::selection::Selection;
use crate::term::Terminal;

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl<'a> Canvas<'a> {
    #[inline]
    pub fn clear(&mut self, c: Rgb) {
        self.buf.fill(c.pack());
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
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

    /// Blend an 8-bit coverage mask in `fg` over whatever is already there.
    fn blit_mask(&mut self, data: &[u8], gw: usize, gh: usize, ox: i32, oy: i32, fg: Rgb) {
        let (fr, fg_, fb) = (fg.0 as u32, fg.1 as u32, fg.2 as u32);
        for row in 0..gh {
            let y = oy + row as i32;
            if y < 0 || y as usize >= self.height {
                continue;
            }
            let dst_base = y as usize * self.width;
            let src_base = row * gw;
            for col in 0..gw {
                let a = data[src_base + col] as u32;
                if a == 0 {
                    continue;
                }
                let x = ox + col as i32;
                if x < 0 || x as usize >= self.width {
                    continue;
                }
                let d = &mut self.buf[dst_base + x as usize];
                if a == 255 {
                    *d = (fr << 16) | (fg_ << 8) | fb;
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

    /// Blend premultiplied-ish BGRA colour bitmap data (colour emoji).
    fn blit_color(&mut self, data: &[u8], gw: usize, gh: usize, ox: i32, oy: i32) {
        for row in 0..gh {
            let y = oy + row as i32;
            if y < 0 || y as usize >= self.height {
                continue;
            }
            let dst_base = y as usize * self.width;
            for col in 0..gw {
                let i = (row * gw + col) * 4;
                let a = data[i + 3] as u32;
                if a == 0 {
                    continue;
                }
                let x = ox + col as i32;
                if x < 0 || x as usize >= self.width {
                    continue;
                }
                let (sr, sg, sb) = (data[i] as u32, data[i + 1] as u32, data[i + 2] as u32);
                let d = &mut self.buf[dst_base + x as usize];
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
    let mut h = std::collections::hash_map::DefaultHasher::new();
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
}

impl Renderer {
    /// Layout of a visible row from the last frame, for mouse hit-testing.
    pub fn row_layout(&self, row: usize) -> Option<&RowLayout> {
        self.layouts.get(row).and_then(|k| self.cache.get(k))
    }

    pub fn new() -> Self {
        Renderer {
            layouter: Layouter::new(),
            layouts: Vec::new(),
            pad_x: 4.0,
            pad_y: 2.0,
            cache: std::collections::HashMap::new(),
            live: Vec::new(),
        }
    }

    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        term: &Terminal,
        fonts: &mut Fonts,
        palette: &Palette,
        selection: Option<Selection>,
        focused: bool,
    ) {
        let reverse = term.modes.contains(crate::term::Modes::REVERSE_VIDEO);
        let (def_fg, def_bg) =
            if reverse { (palette.bg, palette.fg) } else { (palette.fg, palette.bg) };
        canvas.clear(def_bg);

        let cell_w = fonts.cell_w;
        let cell_h = fonts.cell_h;
        let baseline = fonts.baseline;
        let cols = term.grid.cols;
        let rows = term.grid.rows;

        let resolve = |c: Color, is_fg: bool| -> Rgb {
            match c {
                Color::Default => {
                    if is_fg {
                        def_fg
                    } else {
                        def_bg
                    }
                }
                other => palette.resolve(other, is_fg),
            }
        };

        let mut keys: Vec<u64> = Vec::with_capacity(rows);
        self.live.clear();
        let bidi_on = term.modes.contains(crate::term::Modes::BIDI);

        for r in 0..rows {
            let row = term.grid.viewport_row(r);
            // Joining context across a soft wrap: a word broken by the right
            // margin has not ended, so its letters must still take their joined
            // forms on both sides of the break.
            let pre = if r > 0 && term.grid.viewport_row(r - 1).wrapped {
                row_tail(term.grid.viewport_row(r - 1), cols)
            } else {
                String::new()
            };
            let post = if row.wrapped && r + 1 < rows {
                row_head(term.grid.viewport_row(r + 1), cols)
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
            let y = self.pad_y + r as f32 * cell_h;

            // --- backgrounds, in visual column order ---
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(cell) = row.cells.get(cell_idx) else { continue };
                let selected = selection.map_or(false, |s| s.contains(r, cell_idx));
                let inverse = cell.attrs.contains(Attrs::INVERSE) ^ reverse;
                let mut bg = if inverse {
                    resolve(cell.fg, true)
                } else {
                    resolve(cell.bg, false)
                };
                if selected {
                    bg = palette.selection_bg;
                }
                if bg != def_bg || selected {
                    canvas.fill_rect(self.pad_x + col as f32 * cell_w, y, cell_w, cell_h, bg);
                }
            }

            // --- procedural box drawing, per visual column ---
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(cell) = row.cells.get(cell_idx) else { continue };
                if !crate::boxdraw::is_procedural(cell.ch) {
                    continue;
                }
                let inverse = cell.attrs.contains(Attrs::INVERSE) ^ reverse;
                let fg = if inverse {
                    resolve(cell.bg, false)
                } else {
                    resolve(cell.fg, true)
                };
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
                let inverse = cell.attrs.contains(Attrs::INVERSE) ^ reverse;
                let selected = selection.map_or(false, |s| s.contains(r, g.cell));
                let mut fg = if inverse {
                    resolve(cell.bg, false)
                } else {
                    resolve(cell.fg, true)
                };
                if selected {
                    if let Some(sfg) = palette.selection_fg {
                        fg = sfg;
                    }
                }
                if cell.attrs.contains(Attrs::DIM) {
                    fg = Rgb(fg.0 / 2 + 8, fg.1 / 2 + 8, fg.2 / 2 + 8);
                }

                let gx = self.pad_x + g.x;
                let gy = y + baseline - g.y;
                let Some(raster) = fonts.rasterize(g.font, g.glyph, g.px, gx) else { continue };
                let ox = gx.floor() as i32 + raster.left;
                let oy = gy.floor() as i32 - raster.top;
                if raster.color {
                    canvas.blit_color(&raster.data, raster.width as usize, raster.height as usize, ox, oy);
                } else {
                    canvas.blit_mask(
                        &raster.data,
                        raster.width as usize,
                        raster.height as usize,
                        ox,
                        oy,
                        fg,
                    );
                }
            }

            // --- underline / strikeout, drawn per visual column so they run
            // continuously under reordered text ---
            for (col, &cell_idx) in layout.visual_cells.iter().enumerate() {
                let Some(cell) = row.cells.get(cell_idx) else { continue };
                if cell.attrs.intersects(Attrs::UNDERLINE | Attrs::STRIKEOUT) {
                    let fg = resolve(cell.fg, true);
                    let x = self.pad_x + col as f32 * cell_w;
                    if cell.attrs.contains(Attrs::UNDERLINE) {
                        canvas.fill_rect(x, y + baseline + 2.0, cell_w, 1.0, fg);
                    }
                    if cell.attrs.contains(Attrs::STRIKEOUT) {
                        canvas.fill_rect(x, y + baseline - cell_h * 0.25, cell_w, 1.0, fg);
                    }
                }
            }

            keys.push(key);
        }

        self.layouts = keys;
        // Evict layouts no longer on screen, so scrollback does not accumulate.
        if self.cache.len() > self.layouts.len() * 4 + 256 {
            let live: std::collections::HashSet<u64> = self.layouts.iter().copied().collect();
            self.cache.retain(|k, _| live.contains(k));
        }

        // --- cursor, mapped through the row's logical->visual column map ---
        if term.modes.contains(crate::term::Modes::CURSOR_VISIBLE) && term.grid.view_offset == 0 {
            let cur = term.grid.screen.cursor;
            if cur.row < rows {
                let Some(layout) = self.layouts.get(cur.row).and_then(|k| self.cache.get(k))
                else {
                    return;
                };
                let col = layout.column_of(cur.col.min(cols.saturating_sub(1)));
                let x = self.pad_x + col as f32 * cell_w;
                let y = self.pad_y + cur.row as f32 * cell_h;
                if focused {
                    canvas.fill_rect(x, y, cell_w, cell_h, palette.cursor);
                    // Re-draw the glyph(s) of that cell in the background
                    // colour so the character stays legible under the block.
                    let row = term.grid.viewport_row(cur.row);
                    let glyphs: Vec<_> =
                        layout.glyphs.iter().filter(|g| g.cell == cur.col).copied().collect();
                    for g in glyphs {
                        let gy = y + baseline - g.y;
                        let gx = self.pad_x + g.x;
                        if let Some(raster) = fonts.rasterize(g.font, g.glyph, g.px, gx) {
                            let ox = gx.floor() as i32 + raster.left;
                            let oy = gy.floor() as i32 - raster.top;
                            if !raster.color {
                                canvas.blit_mask(
                                    &raster.data,
                                    raster.width as usize,
                                    raster.height as usize,
                                    ox,
                                    oy,
                                    def_bg,
                                );
                            }
                        }
                    }
                    let _ = row;
                } else {
                    // Hollow box when unfocused.
                    canvas.fill_rect(x, y, cell_w, 1.0, palette.cursor);
                    canvas.fill_rect(x, y + cell_h - 1.0, cell_w, 1.0, palette.cursor);
                    canvas.fill_rect(x, y, 1.0, cell_h, palette.cursor);
                    canvas.fill_rect(x + cell_w - 1.0, y, 1.0, cell_h, palette.cursor);
                }
                // A thin marker on the leading edge shows the writing direction
                // the cursor will advance in — invaluable in mixed text.
                if layout.rtl {
                    canvas.fill_rect(x, y, 2.0, cell_h * 0.35, palette.selection_bg);
                } else {
                    canvas.fill_rect(x + cell_w - 2.0, y, 2.0, cell_h * 0.35, palette.selection_bg);
                }
            }
        }
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
