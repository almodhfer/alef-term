//! Terminal state: VT/ANSI interpretation on top of the logical grid.

use crate::color::{Color, Rgb};
use crate::grid::{Attrs, Cell, Grid, Row, Screen};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

/// An image placed on the screen, pinned to the absolute line it was drawn on.
pub struct Placed {
    /// Absolute line (grid.scrolled + screen row) of the image's top edge.
    pub line: u64,
    /// Column of its left edge.
    pub col: usize,
    /// Cell width in pixels at the time it was placed.
    pub cell_w: f32,
    pub image: crate::sixel::Image,
}

bitflags::bitflags! {
    /// DEC private and ANSI modes we actually act on.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Modes: u32 {
        const AUTOWRAP        = 1 << 0;  // DECAWM  (?7)
        const CURSOR_VISIBLE  = 1 << 1;  // DECTCEM (?25)
        const ORIGIN          = 1 << 2;  // DECOM   (?6)
        const APP_CURSOR      = 1 << 3;  // DECCKM  (?1)
        const APP_KEYPAD      = 1 << 4;  // DECNKM  (?66)
        const BRACKETED_PASTE = 1 << 5;  // (?2004)
        const MOUSE_CLICK     = 1 << 6;  // (?1000)
        const MOUSE_DRAG      = 1 << 7;  // (?1002)
        const MOUSE_MOTION    = 1 << 8;  // (?1003)
        const MOUSE_SGR       = 1 << 9;  // (?1006)
        const FOCUS_EVENTS    = 1 << 10; // (?1004)
        const REVERSE_VIDEO   = 1 << 11; // DECSCNM (?5)
        const INSERT          = 1 << 12; // IRM     (4)
        const LNM             = 1 << 13; // (20) newline mode
        /// Terminal bidi: reorder rows for display (our own default-on mode,
        /// toggled by the private mode ?8888 so scripts can opt out).
        const BIDI            = 1 << 14;
        /// Turn the wheel into cursor keys on the alternate screen (?1007).
        const ALT_SCROLL      = 1 << 15;
        /// Synchronized output (?2026): hold frames until the update is done.
        const SYNC_OUTPUT     = 1 << 16;
    }
}

impl Default for Modes {
    fn default() -> Self {
        Modes::AUTOWRAP | Modes::CURSOR_VISIBLE | Modes::BIDI | Modes::ALT_SCROLL
    }
}

/// Base paragraph direction policy for rendering.
///
/// The default is deliberately **not** auto-detection. Deriving base direction
/// from row content makes a line's alignment a function of what happens to be
/// on it: a prompt jumps between edges as you type, `git log --graph` loses its
/// gutter, and any table whose first strong character is Arabic flips whole.
/// Pinning the base to LTR keeps the frame still while RTL runs reorder inside
/// it — which is what actually reads correctly. A fully-RTL workflow can select
/// `ForceRtl`, and applications can still request RTL inline with RLM/RLE/RLI.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BidiMode {
    /// Base level pinned to LTR; RTL runs still reorder within the line.
    #[default]
    ForceLtr,
    /// Base level pinned to RTL: the whole screen reads right-to-left.
    ForceRtl,
    /// Detect per row from the first strong character (UAX#9 P2/P3).
    Auto,
}

/// Cursor shape, from DECSCUSR (`CSI Ps SP q`) or the configuration.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// What an application asked the cursor to look like.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CursorStyle {
    pub shape: CursorShape,
    /// `None` when the application has not said, so the configuration decides.
    pub blink: Option<bool>,
}

pub struct Terminal {
    pub grid: Grid,
    pub modes: Modes,
    /// Colours: owned here so OSC 4/10/11/12 can change and report them.
    pub palette: crate::color::Palette,
    /// The configured colours, restored by OSC 104/110/111/112.
    default_palette: crate::color::Palette,
    pub cursor_style: CursorStyle,
    /// BEL characters received since the UI last looked.
    pub bells: u32,
    pub bidi_mode: BidiMode,
    pub title: String,
    /// Bytes to send back up the PTY (device reports, mouse, focus events).
    pub replies: Vec<u8>,
    /// Set whenever the visible content changed and a redraw is needed.
    pub damaged: bool,
    /// Images placed by sixel, pinned to the absolute line they were drawn on
    /// so they scroll with their text.
    pub images: Vec<Placed>,
    /// Kitty graphics protocol state.
    graphics: crate::kitty_graphics::Decoder,
    /// Buffer for the DCS payload currently being collected.
    dcs: Vec<u8>,
    /// True while the collected DCS is a sixel sequence we intend to decode.
    dcs_is_sixel: bool,
    /// Kitty keyboard protocol flag stack. Applications push a set of flags
    /// while they want richer key reporting and pop it on exit, so nesting
    /// (an editor inside a multiplexer) restores the outer program's mode.
    kitty_keyboard: Vec<u8>,
    /// Text an application asked to place on the clipboard via OSC 52. The UI
    /// drains this; reads are refused, see `osc_dispatch`.
    pub clipboard_write: Option<String>,
    /// Current OSC 8 hyperlink, applied to printed cells.
    link: Option<Box<str>>,
    /// True when charset G0 is the DEC line-drawing set (ESC ( 0).
    dec_graphics: bool,
    tab_stops: Vec<bool>,
    /// Pixel size of a cell, needed to lay images out in rows.
    cell_px: (f32, f32),
}

impl Terminal {
    pub fn new(rows: usize, cols: usize, scrollback: usize) -> Self {
        Terminal {
            grid: Grid::new(rows, cols, scrollback),
            modes: Modes::default(),
            palette: crate::color::Palette::dark(),
            default_palette: crate::color::Palette::dark(),
            cursor_style: CursorStyle::default(),
            bells: 0,
            bidi_mode: BidiMode::default(),
            title: String::from("arabic-term"),
            replies: Vec::new(),
            damaged: true,
            images: Vec::new(),
            graphics: Default::default(),
            dcs: Vec::new(),
            dcs_is_sixel: false,
            kitty_keyboard: Vec::new(),
            clipboard_write: None,
            link: None,
            dec_graphics: false,
            tab_stops: default_tabs(cols),
            cell_px: (8.0, 16.0),
        }
    }

    /// Currently active kitty keyboard flags, 0 when the protocol is off.
    #[inline]
    pub fn kitty_flags(&self) -> u8 {
        self.kitty_keyboard.last().copied().unwrap_or(0)
    }

    /// Handle an APC payload — currently only the kitty graphics protocol.
    pub fn handle_apc(&mut self, payload: &[u8]) {
        match self.graphics.feed(payload) {
            crate::kitty_graphics::Command::Display(img) => self.place_image(img),
            crate::kitty_graphics::Command::Delete(id) => {
                match id {
                    Some(_) => {}
                    None => self.images.clear(),
                }
                self.damaged = true;
            }
            crate::kitty_graphics::Command::None => {}
        }
    }

    /// Place a decoded image at the cursor and advance past it.
    fn place_image(&mut self, img: crate::sixel::Image) {
        let (cw, ch) = self.cell_px;
        let rows_tall = ((img.height as f32 / ch.max(1.0)).ceil() as usize).max(1);
        let col = self.grid.screen.cursor.col;
        let line = self.grid.scrolled + self.grid.screen.cursor.row as u64;

        // Bound how many images we retain; old ones are dropped oldest-first.
        if self.images.len() >= 256 {
            self.images.remove(0);
        }
        self.images.push(Placed { line, col, cell_w: cw, image: img });

        // The cursor moves below the image, scrolling if it runs off the bottom.
        for _ in 0..rows_tall {
            self.linefeed();
        }
        self.grid.screen.cursor.col = 0;
        self.damaged = true;
    }

    /// Tell the terminal the pixel size of a cell, so images can be measured
    /// in rows. The UI updates this when the font size changes.
    pub fn set_cell_px(&mut self, w: f32, h: f32) {
        self.cell_px = (w.max(1.0), h.max(1.0));
    }

    /// Drop images that have scrolled beyond the retained scrollback.
    pub fn prune_images(&mut self) {
        let oldest = self.grid.scrolled.saturating_sub(self.grid.max_scrollback as u64);
        self.images.retain(|p| p.line + 64 >= oldest);
    }

    /// Install the configured colours, as both the live palette and the one
    /// OSC resets return to.
    pub fn set_palette(&mut self, p: crate::color::Palette) {
        self.palette = p.clone();
        self.default_palette = p;
        self.damaged = true;
    }

    /// Answer an OSC colour query in xterm's `rgb:rrrr/gggg/bbbb` form,
    /// terminated the same way the query was.
    fn reply_color(&mut self, cmd: &str, c: Rgb, bell_terminated: bool) {
        let end = if bell_terminated { "\x07" } else { "\x1b\\" };
        let s = format!(
            "\x1b]{cmd};rgb:{:04x}/{:04x}/{:04x}{end}",
            c.0 as u32 * 257,
            c.1 as u32 * 257,
            c.2 as u32 * 257
        );
        self.replies.extend_from_slice(s.as_bytes());
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.grid.resize(rows, cols);
        self.tab_stops = default_tabs(cols);
        self.damaged = true;
    }

    #[inline]
    fn screen(&mut self) -> &mut Screen {
        &mut self.grid.screen
    }

    #[inline]
    fn cols(&self) -> usize {
        self.grid.cols
    }

    #[inline]
    fn rows(&self) -> usize {
        self.grid.rows
    }

    /// Cursor row bounds, honouring DECOM (origin mode confines to the region).
    fn row_bounds(&self) -> (usize, usize) {
        if self.modes.contains(Modes::ORIGIN) {
            (self.grid.screen.scroll_top, self.grid.screen.scroll_bot)
        } else {
            (0, self.rows())
        }
    }

    fn goto(&mut self, row: usize, col: usize) {
        let (top, bot) = self.row_bounds();
        let cols = self.cols();
        let row = (top + row).min(bot.saturating_sub(1));
        self.screen().cursor.row = row;
        self.screen().cursor.col = col.min(cols.saturating_sub(1));
        self.screen().cursor.pending_wrap = false;
    }

    fn linefeed(&mut self) {
        let bot = self.grid.screen.scroll_bot;
        let bg = self.grid.screen.cursor.bg;
        if self.grid.screen.cursor.row + 1 >= bot {
            self.grid.scroll_up(1, bg);
        } else {
            self.grid.screen.cursor.row += 1;
        }
        self.grid.screen.cursor.pending_wrap = false;
    }

    /// Reverse index: move up, scrolling the region down at the top margin.
    fn reverse_index(&mut self) {
        let top = self.grid.screen.scroll_top;
        let bg = self.grid.screen.cursor.bg;
        if self.grid.screen.cursor.row == top {
            self.grid.scroll_down(1, bg);
        } else {
            self.grid.screen.cursor.row = self.grid.screen.cursor.row.saturating_sub(1);
        }
    }

    fn wrap_line(&mut self) {
        let row = self.grid.screen.cursor.row;
        self.grid.screen.rows[row].wrapped = true;
        self.grid.screen.cursor.col = 0;
        self.linefeed();
    }

    fn enter_alt_screen(&mut self) {
        if self.grid.alt.is_some() {
            return;
        }
        let rows = self.rows();
        let cols = self.cols();
        let fresh = Screen::new(rows, cols);
        let primary = std::mem::replace(&mut self.grid.screen, fresh);
        self.grid.alt = Some(primary);
        self.grid.view_offset = 0;
    }

    fn leave_alt_screen(&mut self) {
        if let Some(primary) = self.grid.alt.take() {
            self.grid.screen = primary;
        }
        self.grid.view_offset = 0;
    }

    /// Erase a span of the current row, inheriting the active background.
    fn erase_in_row(&mut self, row: usize, range: std::ops::Range<usize>) {
        let bg = self.grid.screen.cursor.bg;
        let cols = self.cols();
        if let Some(r) = self.grid.screen.rows.get_mut(row) {
            for c in &mut r.cells[range.start.min(cols)..range.end.min(cols)] {
                c.reset(bg);
            }
        }
    }

    /// The flag a private mode number maps to, for reporting purposes.
    ///
    /// Kept separate from `set_mode`, which applies side effects. Anything not
    /// listed is reported as unrecognised rather than guessed at.
    fn private_mode_flag(n: u16) -> Option<Modes> {
        Some(match n {
            1 => Modes::APP_CURSOR,
            5 => Modes::REVERSE_VIDEO,
            6 => Modes::ORIGIN,
            7 => Modes::AUTOWRAP,
            25 => Modes::CURSOR_VISIBLE,
            66 => Modes::APP_KEYPAD,
            1000 => Modes::MOUSE_CLICK,
            1002 => Modes::MOUSE_DRAG,
            1003 => Modes::MOUSE_MOTION,
            1004 => Modes::FOCUS_EVENTS,
            1006 => Modes::MOUSE_SGR,
            1007 => Modes::ALT_SCROLL,
            2004 => Modes::BRACKETED_PASTE,
            2026 => Modes::SYNC_OUTPUT,
            8888 => Modes::BIDI,
            _ => return None,
        })
    }

    /// DECRQM: report whether a mode is set, reset, or unknown to us.
    ///
    /// Applications ask before using a feature. Without an answer they assume
    /// it is missing — synchronized output in particular is only ever sent by
    /// programs that first confirmed the terminal understands it, so failing
    /// to reply here makes the feature dead code.
    fn report_mode(&mut self, private: bool, n: u16) {
        let state = if private {
            match n {
                1049 | 47 | 1047 => {
                    if self.grid.in_alt() {
                        1
                    } else {
                        2
                    }
                }
                _ => match Self::private_mode_flag(n) {
                    Some(flag) => {
                        if self.modes.contains(flag) {
                            1
                        } else {
                            2
                        }
                    }
                    // 0: the terminal does not recognise the mode.
                    None => 0,
                },
            }
        } else {
            match n {
                4 => {
                    if self.modes.contains(Modes::INSERT) {
                        1
                    } else {
                        2
                    }
                }
                20 => {
                    if self.modes.contains(Modes::LNM) {
                        1
                    } else {
                        2
                    }
                }
                _ => 0,
            }
        };
        let q = if private { "?" } else { "" };
        self.replies.extend_from_slice(format!("\x1b[{q}{n};{state}$y").as_bytes());
    }

    fn set_mode(&mut self, private: bool, n: u16, on: bool) {
        let flag = match (private, n) {
            (true, 1) => Modes::APP_CURSOR,
            (true, 5) => Modes::REVERSE_VIDEO,
            (true, 6) => {
                // DECOM also homes the cursor into the region.
                self.modes.set(Modes::ORIGIN, on);
                let top = if on { self.grid.screen.scroll_top } else { 0 };
                self.grid.screen.cursor.row = top;
                self.grid.screen.cursor.col = 0;
                return;
            }
            (true, 7) => Modes::AUTOWRAP,
            (true, 25) => Modes::CURSOR_VISIBLE,
            (true, 66) => Modes::APP_KEYPAD,
            (true, 1000) => Modes::MOUSE_CLICK,
            (true, 1002) => Modes::MOUSE_DRAG,
            (true, 1003) => Modes::MOUSE_MOTION,
            (true, 1004) => Modes::FOCUS_EVENTS,
            (true, 1006) => Modes::MOUSE_SGR,
            (true, 1007) => Modes::ALT_SCROLL,
            (true, 2026) => Modes::SYNC_OUTPUT,
            (true, 2004) => Modes::BRACKETED_PASTE,
            (true, 8888) => Modes::BIDI,
            (true, 47) | (true, 1047) | (true, 1049) => {
                if on {
                    if n == 1049 {
                        self.grid.screen.saved_cursor = self.grid.screen.cursor;
                    }
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                    if n == 1049 {
                        self.grid.screen.cursor = self.grid.screen.saved_cursor;
                    }
                }
                self.damaged = true;
                return;
            }
            (true, 1048) => {
                if on {
                    self.grid.screen.saved_cursor = self.grid.screen.cursor;
                } else {
                    self.grid.screen.cursor = self.grid.screen.saved_cursor;
                }
                return;
            }
            (false, 4) => Modes::INSERT,
            (false, 20) => Modes::LNM,
            _ => return,
        };
        self.modes.set(flag, on);
    }

    fn sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        // A bare `ESC [ m` is SGR 0.
        if params.is_empty() {
            let c = &mut self.grid.screen.cursor;
            c.attrs = Attrs::empty();
            c.fg = Color::Default;
            c.bg = Color::Default;
            return;
        }
        while let Some(p) = iter.next() {
            let n = p[0];
            // Colon-form (38:2::r:g:b / 38:5:n) carries subparams in the same
            // slice; semicolon-form spreads them over the following params.
            match n {
                38 | 48 | 58 => {
                    let color = if p.len() > 1 {
                        parse_color_subparams(&p[1..])
                    } else {
                        parse_color_semicolon(&mut iter)
                    };
                    if let Some(col) = color {
                        let c = &mut self.grid.screen.cursor;
                        match n {
                            38 => c.fg = col,
                            48 => c.bg = col,
                            _ => {}
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let c = &mut self.grid.screen.cursor;
            match n {
                0 => {
                    c.attrs = Attrs::empty();
                    c.fg = Color::Default;
                    c.bg = Color::Default;
                }
                1 => c.attrs.insert(Attrs::BOLD),
                2 => c.attrs.insert(Attrs::DIM),
                3 => c.attrs.insert(Attrs::ITALIC),
                4 => c.attrs.insert(Attrs::UNDERLINE),
                5 | 6 => c.attrs.insert(Attrs::BLINK),
                7 => c.attrs.insert(Attrs::INVERSE),
                8 => c.attrs.insert(Attrs::HIDDEN),
                9 => c.attrs.insert(Attrs::STRIKEOUT),
                21 | 22 => c.attrs.remove(Attrs::BOLD | Attrs::DIM),
                23 => c.attrs.remove(Attrs::ITALIC),
                24 => c.attrs.remove(Attrs::UNDERLINE),
                25 => c.attrs.remove(Attrs::BLINK),
                27 => c.attrs.remove(Attrs::INVERSE),
                28 => c.attrs.remove(Attrs::HIDDEN),
                29 => c.attrs.remove(Attrs::STRIKEOUT),
                30..=37 => c.fg = Color::Indexed((n - 30) as u8),
                39 => c.fg = Color::Default,
                40..=47 => c.bg = Color::Indexed((n - 40) as u8),
                49 => c.bg = Color::Default,
                90..=97 => c.fg = Color::Indexed((n - 90 + 8) as u8),
                100..=107 => c.bg = Color::Indexed((n - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|i| i % 8 == 0 && i != 0).collect()
}

/// `38:2::r:g:b`, `38:2:r:g:b` or `38:5:n` — subparams within one param slice.
fn parse_color_subparams(sub: &[u16]) -> Option<Color> {
    match sub.first()? {
        2 => {
            // Some emitters include a colour-space id, some do not.
            let rgb = if sub.len() >= 5 { &sub[2..5] } else { sub.get(1..4)? };
            Some(Color::Rgb(Rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8)))
        }
        5 => Some(Color::Indexed(*sub.get(1)? as u8)),
        _ => None,
    }
}

/// `38;2;r;g;b` or `38;5;n` — subparams spread across following params.
fn parse_color_semicolon<'a>(iter: &mut vte::ParamsIter<'a>) -> Option<Color> {
    match iter.next()?[0] {
        2 => {
            let r = iter.next()?[0] as u8;
            let g = iter.next()?[0] as u8;
            let b = iter.next()?[0] as u8;
            Some(Color::Rgb(Rgb(r, g, b)))
        }
        5 => Some(Color::Indexed(iter.next()?[0] as u8)),
        _ => None,
    }
}

/// Undo pre-shaped Arabic.
///
/// Editors that predate shaping terminals — vim and neovim do this by default,
/// via `'arabicshape'` — convert Arabic to Arabic Presentation Forms before
/// writing it out, because they assume the terminal cannot join letters itself.
/// Those codepoints are frozen isolated/initial/medial/final shapes, so feeding
/// them to a real shaper produces disconnected letters: exactly the breakage
/// this terminal exists to avoid.
///
/// Mapping each one back to its base letter lets the shaper do the joining
/// properly, and makes copied text ordinary Arabic rather than presentation
/// forms, which Unicode discourages for interchange.
///
/// Forms that decompose to *several* characters are ligatures — lam-alef and
/// friends. Those are left alone: they are already a single correct glyph
/// sitting in a single cell, and expanding them would need more cells than the
/// program reserved.
#[inline]
fn unshape_presentation_form(c: char) -> char {
    if !matches!(c as u32, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF) {
        return c;
    }
    let mut out = None;
    let mut count = 0usize;
    unicode_normalization::char::decompose_compatible(c, |d| {
        count += 1;
        if count == 1 {
            out = Some(d);
        }
    });
    match out {
        Some(base) if count == 1 => base,
        _ => c,
    }
}

/// Map ASCII to the DEC special graphics set (box drawing) when ESC ( 0 is active.
fn dec_graphic(c: char) -> char {
    match c {
        '`' => '◆', 'a' => '▒', 'b' => '\u{2409}', 'c' => '\u{240c}',
        'd' => '\u{240d}', 'e' => '\u{240a}', 'f' => '°', 'g' => '±',
        'h' => '\u{2424}', 'i' => '\u{240b}', 'j' => '┘', 'k' => '┐',
        'l' => '┌', 'm' => '└', 'n' => '┼', 'o' => '⎺', 'p' => '⎻',
        'q' => '─', 'r' => '⎼', 's' => '⎽', 't' => '├', 'u' => '┤',
        'v' => '┴', 'w' => '┬', 'x' => '│', 'y' => '≤', 'z' => '≥',
        '{' => 'π', '|' => '≠', '}' => '£', '~' => '·',
        _ => c,
    }
}

impl Perform for Terminal {
    fn print(&mut self, c: char) {
        self.damaged = true;
        let c = if self.dec_graphics { dec_graphic(c) } else { c };
        let c = unshape_presentation_form(c);
        let width = c.width().unwrap_or(0);

        // Zero-width: a combining mark belongs to the cell already written.
        // Arabic harakat land here, which is what keeps them off the grid as
        // separate columns while still reaching the shaper.
        if width == 0 {
            let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
            let cols = self.cols();
            let target = if col == 0 { 0 } else { col.min(cols - 1) };
            // Step back over the spacer half of a wide character.
            let r = &mut self.grid.screen.rows[row];
            let mut idx = target;
            if idx > 0 && r.cells[idx].ch == ' ' && r.cells[idx].attrs.contains(Attrs::WIDE_SPACER)
            {
                idx -= 1;
            }
            if self.grid.screen.cursor.col > 0 && !self.grid.screen.cursor.pending_wrap {
                idx = idx.saturating_sub(1);
            }
            r.cells[idx].push_mark(c);
            return;
        }

        let cols = self.cols();
        if self.grid.screen.cursor.pending_wrap && self.modes.contains(Modes::AUTOWRAP) {
            self.wrap_line();
        }
        // A double-width character will not straddle the right margin.
        if width == 2 && self.grid.screen.cursor.col + 1 >= cols {
            if self.modes.contains(Modes::AUTOWRAP) {
                self.wrap_line();
            } else {
                return;
            }
        }

        if self.modes.contains(Modes::INSERT) {
            let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
            let r = &mut self.grid.screen.rows[row];
            for _ in 0..width {
                r.cells.insert(col, Cell::default());
            }
            r.cells.truncate(cols);
        }

        let cur = self.grid.screen.cursor;
        let row = cur.row;
        let col = cur.col;
        let link = self.link.clone();
        {
            let r = &mut self.grid.screen.rows[row];
            let cell = &mut r.cells[col];
            cell.ch = c;
            cell.fg = cur.fg;
            cell.bg = cur.bg;
            cell.attrs = cur.attrs;
            cell.extra = None;
            if width == 2 {
                cell.attrs.insert(Attrs::WIDE_LEADING);
            }
            if let Some(l) = link {
                cell.extra.get_or_insert_with(Default::default).link = Some(l);
            }
            if width == 2 && col + 1 < cols {
                let spacer = &mut r.cells[col + 1];
                spacer.reset(cur.bg);
                spacer.attrs = cur.attrs | Attrs::WIDE_SPACER;
                spacer.fg = cur.fg;
            }
        }

        let next = col + width;
        if next >= cols {
            self.grid.screen.cursor.col = cols - 1;
            self.grid.screen.cursor.pending_wrap = true;
        } else {
            self.grid.screen.cursor.col = next;
        }
    }

    fn execute(&mut self, byte: u8) {
        self.damaged = true;
        match byte {
            0x07 => self.bells = self.bells.saturating_add(1),
            0x08 => {
                // Backspace: undo a pending wrap first, then step left.
                if self.grid.screen.cursor.pending_wrap {
                    self.grid.screen.cursor.pending_wrap = false;
                } else {
                    self.grid.screen.cursor.col = self.grid.screen.cursor.col.saturating_sub(1);
                }
            }
            0x09 => {
                let cols = self.cols();
                let mut c = self.grid.screen.cursor.col + 1;
                while c < cols && !self.tab_stops.get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                self.grid.screen.cursor.col = c.min(cols - 1);
                self.grid.screen.cursor.pending_wrap = false;
            }
            0x0a | 0x0b | 0x0c => {
                self.linefeed();
                if self.modes.contains(Modes::LNM) {
                    self.grid.screen.cursor.col = 0;
                }
            }
            0x0d => {
                self.grid.screen.cursor.col = 0;
                self.grid.screen.cursor.pending_wrap = false;
            }
            0x0e => self.dec_graphics = true,  // SO
            0x0f => self.dec_graphics = false, // SI
            _ => {}
        }
    }

    fn hook(&mut self, _p: &Params, intermediates: &[u8], _ig: bool, action: char) {
        // Sixel is `DCS <params> q` with no intermediate. `DCS $ q` is DECRQSS
        // asking for a setting back, and shares the same final byte — checking
        // only that would turn every status request into a broken image.
        self.dcs.clear();
        self.dcs_is_sixel = action == 'q' && intermediates.is_empty();
    }

    fn put(&mut self, byte: u8) {
        // Cap the payload so a stream that never terminates cannot grow it
        // without bound.
        if self.dcs.len() < 16 << 20 {
            self.dcs.push(byte);
        }
    }

    fn unhook(&mut self) {
        if !self.dcs_is_sixel {
            self.dcs.clear();
            return;
        }
        let data = std::mem::take(&mut self.dcs);
        self.dcs_is_sixel = false;
        let Some(img) = crate::sixel::decode(&data) else { return };
        self.place_image(img);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let Some(cmd) = params.first() else { return };
        match *cmd {
            b"0" | b"1" | b"2" => {
                if let Some(t) = params.get(1) {
                    self.title = String::from_utf8_lossy(t).into_owned();
                }
            }
            b"4" => {
                // OSC 4 ; index ; rgb:RR/GG/BB — or `?` to ask.
                let mut i = 1;
                while i + 1 < params.len() {
                    let idx = std::str::from_utf8(params[i]).unwrap_or("").parse::<u16>();
                    match (idx, params[i + 1]) {
                        (Ok(idx), b"?") if idx < 256 => {
                            let c = self.palette.colors[idx as usize];
                            self.reply_color(&format!("4;{idx}"), c, bell_terminated);
                        }
                        (Ok(idx), spec) if idx < 256 => {
                            if let Some(rgb) = parse_xcolor(spec) {
                                self.palette.colors[idx as usize] = rgb;
                                self.damaged = true;
                            }
                        }
                        _ => {}
                    }
                    i += 2;
                }
            }
            b"104" => {
                // Reset indexed colours: the listed ones, or all of them.
                if params.len() == 1 {
                    self.palette.colors = self.default_palette.colors;
                }
                for p in &params[1..] {
                    if let Ok(idx) = std::str::from_utf8(p).unwrap_or("").parse::<usize>() {
                        if idx < 256 {
                            self.palette.colors[idx] = self.default_palette.colors[idx];
                        }
                    }
                }
                self.damaged = true;
            }
            b"10" | b"11" | b"12" => {
                // Default foreground, background and cursor colour. Programs
                // query these on startup to pick a light or dark theme, and
                // some hang until they hear back.
                let which = cmd[1] - b'0';
                let Some(spec) = params.get(1) else { return };
                if *spec == b"?" {
                    let c = match which {
                        0 => self.palette.fg,
                        1 => self.palette.bg,
                        _ => self.palette.cursor,
                    };
                    self.reply_color(&format!("1{which}"), c, bell_terminated);
                } else if let Some(rgb) = parse_xcolor(spec) {
                    match which {
                        0 => self.palette.fg = rgb,
                        1 => self.palette.bg = rgb,
                        _ => self.palette.cursor = rgb,
                    }
                    self.damaged = true;
                }
            }
            b"110" => {
                self.palette.fg = self.default_palette.fg;
                self.damaged = true;
            }
            b"111" => {
                self.palette.bg = self.default_palette.bg;
                self.damaged = true;
            }
            b"112" => {
                self.palette.cursor = self.default_palette.cursor;
                self.damaged = true;
            }
            b"52" => {
                // OSC 52: set the clipboard. Params are ; <selection> ; <base64>.
                //
                // Reads are deliberately not implemented. A terminal that
                // answers OSC 52 queries lets any program that can write to the
                // tty — including one on the far end of an ssh session, or
                // anything that manages to echo a string — exfiltrate whatever
                // the user last copied. Every mainstream terminal disables
                // reads by default for this reason.
                let Some(data) = params.get(2) else { return };
                if *data == b"?" {
                    log::debug!("refusing OSC 52 clipboard read");
                    return;
                }
                match base64_decode(data) {
                    Some(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        // Cap it: a runaway sequence should not be able to make
                        // us allocate without bound.
                        if text.len() <= 1 << 20 {
                            self.clipboard_write = Some(text);
                        }
                    }
                    None => log::debug!("ignoring malformed OSC 52 payload"),
                }
            }
            b"8" => {
                // OSC 8 ; params ; URI  (empty URI closes the link)
                self.link = match params.get(2) {
                    Some(uri) if !uri.is_empty() => {
                        Some(String::from_utf8_lossy(uri).into_owned().into_boxed_str())
                    }
                    _ => None,
                };
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.damaged = true;
        let private = intermediates.first() == Some(&b'?');
        // First value of each param, defaulting to 0 when omitted.
        let arg = |i: usize| -> u16 { params.iter().nth(i).map_or(0, |p| p[0]) };
        // Same, but with the ANSI "omitted means 1" convention.
        let arg1 = |i: usize| -> usize { arg(i).max(1) as usize };

        match action {
            'A' => {
                let n = arg1(0);
                let top = self.grid.screen.scroll_top;
                let cur = self.grid.screen.cursor.row;
                self.grid.screen.cursor.row = cur.saturating_sub(n).max(top.min(cur));
                self.grid.screen.cursor.pending_wrap = false;
            }
            'B' => {
                let n = arg1(0);
                let bot = self.grid.screen.scroll_bot;
                self.grid.screen.cursor.row = (self.grid.screen.cursor.row + n).min(bot - 1);
                self.grid.screen.cursor.pending_wrap = false;
            }
            'C' | 'a' => {
                let cols = self.cols();
                self.grid.screen.cursor.col = (self.grid.screen.cursor.col + arg1(0)).min(cols - 1);
                self.grid.screen.cursor.pending_wrap = false;
            }
            'D' => {
                self.grid.screen.cursor.col = self.grid.screen.cursor.col.saturating_sub(arg1(0));
                self.grid.screen.cursor.pending_wrap = false;
            }
            'E' => {
                let n = arg1(0);
                let bot = self.grid.screen.scroll_bot;
                self.grid.screen.cursor.row = (self.grid.screen.cursor.row + n).min(bot - 1);
                self.grid.screen.cursor.col = 0;
            }
            'F' => {
                let n = arg1(0);
                self.grid.screen.cursor.row = self.grid.screen.cursor.row.saturating_sub(n);
                self.grid.screen.cursor.col = 0;
            }
            'G' | '`' => {
                let cols = self.cols();
                self.grid.screen.cursor.col = (arg1(0) - 1).min(cols - 1);
                self.grid.screen.cursor.pending_wrap = false;
            }
            'd' => {
                let r = arg1(0) - 1;
                let rows = self.rows();
                self.grid.screen.cursor.row = r.min(rows - 1);
                self.grid.screen.cursor.pending_wrap = false;
            }
            'H' | 'f' => {
                let (r, c) = (arg1(0) - 1, arg1(1) - 1);
                self.goto(r, c);
            }
            'J' => {
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let (rows, cols) = (self.rows(), self.cols());
                match arg(0) {
                    0 => {
                        self.erase_in_row(row, col..cols);
                        for r in row + 1..rows {
                            self.erase_in_row(r, 0..cols);
                        }
                    }
                    1 => {
                        self.erase_in_row(row, 0..col + 1);
                        for r in 0..row {
                            self.erase_in_row(r, 0..cols);
                        }
                    }
                    2 | 3 => {
                        // ED 3 also clears scrollback.
                        if arg(0) == 3 {
                            self.grid.scrollback.clear();
                            self.grid.view_offset = 0;
                        }
                        for r in 0..rows {
                            self.erase_in_row(r, 0..cols);
                        }
                    }
                    _ => {}
                }
            }
            'K' => {
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let cols = self.cols();
                match arg(0) {
                    0 => self.erase_in_row(row, col..cols),
                    1 => self.erase_in_row(row, 0..col + 1),
                    2 => self.erase_in_row(row, 0..cols),
                    _ => {}
                }
            }
            'L' | 'M' => {
                // IL / DL operate within the scrolling region on the cursor row.
                let row = self.grid.screen.cursor.row;
                let (top, bot) = (self.grid.screen.scroll_top, self.grid.screen.scroll_bot);
                if row < top || row >= bot {
                    return;
                }
                let n = arg1(0).min(bot - row);
                let bg = self.grid.screen.cursor.bg;
                let cols = self.cols();
                for _ in 0..n {
                    if action == 'L' {
                        let mut r = self.grid.screen.rows.remove(bot - 1);
                        r.resize(cols);
                        r.reset(bg);
                        self.grid.screen.rows.insert(row, r);
                    } else {
                        let mut r = self.grid.screen.rows.remove(row);
                        r.resize(cols);
                        r.reset(bg);
                        self.grid.screen.rows.insert(bot - 1, r);
                    }
                }
                self.grid.screen.cursor.col = 0;
            }
            'P' => {
                // DCH
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let cols = self.cols();
                let n = arg1(0).min(cols - col);
                let bg = self.grid.screen.cursor.bg;
                let r = &mut self.grid.screen.rows[row];
                r.cells.drain(col..col + n);
                let mut fill = Cell::default();
                fill.bg = bg;
                r.cells.resize(cols, fill);
            }
            '@' => {
                // ICH
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let cols = self.cols();
                let n = arg1(0).min(cols - col);
                let bg = self.grid.screen.cursor.bg;
                let r = &mut self.grid.screen.rows[row];
                let mut fill = Cell::default();
                fill.bg = bg;
                for _ in 0..n {
                    r.cells.insert(col, fill.clone());
                }
                r.cells.truncate(cols);
            }
            'X' => {
                // ECH
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let cols = self.cols();
                let n = arg1(0);
                self.erase_in_row(row, col..(col + n).min(cols));
            }
            'S' => {
                let bg = self.grid.screen.cursor.bg;
                self.grid.scroll_up(arg1(0), bg);
            }
            'T' => {
                let bg = self.grid.screen.cursor.bg;
                self.grid.scroll_down(arg1(0), bg);
            }
            'b' => {
                // REP: repeat the last printed character.
                let (row, col) = (self.grid.screen.cursor.row, self.grid.screen.cursor.col);
                let prev = col.saturating_sub(1);
                let ch = self.grid.screen.rows[row].cells[prev].ch;
                for _ in 0..arg1(0) {
                    self.print(ch);
                }
            }
            'g' => {
                let col = self.grid.screen.cursor.col;
                match arg(0) {
                    0 => {
                        if let Some(t) = self.tab_stops.get_mut(col) {
                            *t = false;
                        }
                    }
                    3 => self.tab_stops.iter_mut().for_each(|t| *t = false),
                    _ => {}
                }
            }
            'p' if intermediates.contains(&b'$') => {
                // DECRQM — request mode.
                for p in params.iter() {
                    self.report_mode(private, p[0]);
                }
            }
            'h' => {
                for p in params.iter() {
                    self.set_mode(private, p[0], true);
                }
            }
            'l' => {
                for p in params.iter() {
                    self.set_mode(private, p[0], false);
                }
            }
            'm' => self.sgr(params),
            'n' => match arg(0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let (top, _) = self.row_bounds();
                    let r = self.grid.screen.cursor.row - top + 1;
                    let c = self.grid.screen.cursor.col + 1;
                    self.replies.extend_from_slice(format!("\x1b[{r};{c}R").as_bytes());
                }
                _ => {}
            },
            'c' => self.replies.extend_from_slice(b"\x1b[?6c"), // VT102
            'q' if intermediates.contains(&b' ') => {
                // DECSCUSR: 0/1 blinking block, 2 steady block, 3/4 underline,
                // 5/6 bar. Zero means "whatever the terminal's default is".
                let n = arg(0);
                self.cursor_style = match n {
                    0 => CursorStyle::default(),
                    1 | 2 => CursorStyle { shape: CursorShape::Block, blink: Some(n == 1) },
                    3 | 4 => CursorStyle { shape: CursorShape::Underline, blink: Some(n == 3) },
                    5 | 6 => CursorStyle { shape: CursorShape::Bar, blink: Some(n == 5) },
                    _ => return,
                };
            }
            'r' => {
                let rows = self.rows();
                let top = arg1(0) - 1;
                let bot = if arg(1) == 0 { rows } else { arg1(1) };
                if top < bot && bot <= rows {
                    self.grid.screen.scroll_top = top;
                    self.grid.screen.scroll_bot = bot;
                    let (t, _) = self.row_bounds();
                    self.grid.screen.cursor.row = t;
                    self.grid.screen.cursor.col = 0;
                }
            }
            's' => self.grid.screen.saved_cursor = self.grid.screen.cursor,
            'u' => match intermediates.first() {
                // Kitty keyboard protocol. Plain `CSI u` is still restore-cursor.
                Some(b'?') => {
                    let f = self.kitty_flags();
                    self.replies.extend_from_slice(format!("\x1b[?{f}u").as_bytes());
                }
                Some(b'>') => {
                    // Push flags; bound the stack so a hostile stream cannot
                    // grow it without limit.
                    if self.kitty_keyboard.len() < 32 {
                        self.kitty_keyboard.push((arg(0) & 0xff) as u8);
                    }
                }
                Some(b'<') => {
                    for _ in 0..arg1(0) {
                        self.kitty_keyboard.pop();
                    }
                }
                Some(b'=') => {
                    let flags = (arg(0) & 0xff) as u8;
                    let mode = if params.iter().count() > 1 { arg(1) } else { 1 };
                    let cur = self.kitty_flags();
                    let next = match mode {
                        2 => cur | flags,  // set given bits
                        3 => cur & !flags, // clear given bits
                        _ => flags,        // replace
                    };
                    if self.kitty_keyboard.is_empty() {
                        self.kitty_keyboard.push(next);
                    } else {
                        *self.kitty_keyboard.last_mut().unwrap() = next;
                    }
                }
                _ => self.grid.screen.cursor = self.grid.screen.saved_cursor,
            },
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.damaged = true;
        match (intermediates.first(), byte) {
            (None, b'D') => self.linefeed(),
            (None, b'E') => {
                self.linefeed();
                self.grid.screen.cursor.col = 0;
            }
            (None, b'M') => self.reverse_index(),
            (None, b'7') => self.grid.screen.saved_cursor = self.grid.screen.cursor,
            (None, b'8') => self.grid.screen.cursor = self.grid.screen.saved_cursor,
            (None, b'H') => {
                let col = self.grid.screen.cursor.col;
                if let Some(t) = self.tab_stops.get_mut(col) {
                    *t = true;
                }
            }
            (None, b'c') => {
                // RIS: full reset.
                let (rows, cols) = (self.rows(), self.cols());
                let sb = self.grid.max_scrollback;
                *self = Terminal::new(rows, cols, sb);
            }
            (Some(b'('), b'0') => self.dec_graphics = true,
            (Some(b'('), _) => self.dec_graphics = false,
            _ => {}
        }
    }
}

/// Decode standard base64, rejecting anything malformed.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    #[inline]
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn parse_xcolor(spec: &[u8]) -> Option<Rgb> {
    let s = std::str::from_utf8(spec).ok()?;
    let hex = s.strip_prefix("rgb:")?;
    let mut it = hex.split('/');
    let mut comp = || -> Option<u8> {
        let p = it.next()?;
        let v = u32::from_str_radix(p, 16).ok()?;
        // Scale 1/2/4-hex-digit components down to 8 bits.
        Some(match p.len() {
            1 => (v * 17) as u8,
            2 => v as u8,
            4 => (v >> 8) as u8,
            _ => return None,
        })
    };
    Some(Rgb(comp()?, comp()?, comp()?))
}

/// Feed PTY bytes through the parser into terminal state.
///
/// APC sequences (`ESC _ ... ESC \`) are pulled out of the stream first: vte
/// recognises the state but discards the payload without a callback, and the
/// kitty graphics protocol lives there.
pub struct Vt {
    parser: vte::Parser,
    /// Partial APC payload carried across a chunk boundary.
    apc: Vec<u8>,
    in_apc: bool,
    /// Trailing ESC held back in case it begins an APC introducer.
    held_esc: bool,
}

impl Vt {
    pub fn new() -> Self {
        Vt { parser: vte::Parser::new(), apc: Vec::new(), in_apc: false, held_esc: false }
    }

    pub fn feed(&mut self, term: &mut Terminal, bytes: &[u8]) {
        let mut rest = bytes;
        // A held ESC from the previous chunk: decide now what it introduced.
        if self.held_esc {
            self.held_esc = false;
            match rest.first() {
                Some(b'_') => {
                    self.in_apc = true;
                    self.apc.clear();
                    rest = &rest[1..];
                }
                _ => self.parser.advance(term, b"\x1b"),
            }
        }

        while !rest.is_empty() {
            if self.in_apc {
                // Collect until ESC \ (or a bare BEL, which some emitters use).
                match rest.iter().position(|&b| b == 0x1b || b == 0x07) {
                    Some(i) => {
                        self.push_apc(&rest[..i]);
                        if rest[i] == 0x07 {
                            rest = &rest[i + 1..];
                        } else if i + 1 < rest.len() {
                            rest = &rest[i + 2..]; // skip ESC and the following byte
                        } else {
                            // ESC at the very end of the chunk: treat it as the
                            // terminator rather than stalling the image until
                            // more bytes happen to arrive.
                            self.finish_apc(term);
                            break;
                        }
                        self.finish_apc(term);
                    }
                    None => {
                        self.push_apc(rest);
                        break;
                    }
                }
                continue;
            }

            match rest.iter().position(|&b| b == 0x1b) {
                Some(i) => {
                    self.parser.advance(term, &rest[..i]);
                    match rest.get(i + 1) {
                        Some(b'_') => {
                            self.in_apc = true;
                            self.apc.clear();
                            rest = &rest[i + 2..];
                        }
                        Some(_) => {
                            self.parser.advance(term, &rest[i..i + 2]);
                            rest = &rest[i + 2..];
                        }
                        None => {
                            // Hold the ESC; the next chunk decides.
                            self.held_esc = true;
                            break;
                        }
                    }
                }
                None => {
                    self.parser.advance(term, rest);
                    break;
                }
            }
        }
    }

    fn push_apc(&mut self, bytes: &[u8]) {
        if self.apc.len() + bytes.len() <= 32 << 20 {
            self.apc.extend_from_slice(bytes);
        }
    }

    fn finish_apc(&mut self, term: &mut Terminal) {
        self.in_apc = false;
        let payload = std::mem::take(&mut self.apc);
        term.handle_apc(&payload);
    }
}

#[allow(dead_code)]
fn _assert_row(_: &Row) {}
