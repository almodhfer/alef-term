//! Window, input handling, and the frame loop.

use crate::font::Fonts;
use crate::commands::{Action, CommandPalette};
use crate::frame::FramePacer;
use crate::pty::Pty;
use crate::gpu::Gpu;
use crate::color::{find_theme, Theme};
use crate::picker::ThemePicker;
use crate::render::{Canvas, DrawOpts, PickerView, Renderer, SearchBar, TabBar};
use crate::search::{Index, Search};
use crate::selection::{logical_line, selected_text, url_at, word_at, Selection, Span};
use crate::shortcuts::{Platform, Shortcut, PLATFORM};
use crate::term::{BidiMode, CursorShape, Modes, Terminal, Vt};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// A pointer position resolved into grid coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Hit {
    /// Row within the visible viewport, which is what an application is told.
    row: usize,
    /// Absolute line across scrollback and screen, which is what a selection
    /// anchors to.
    line: usize,
    /// Logical cell, mapped back through the row's visual order.
    cell: usize,
}

/// Woken by the PTY reader thread so the UI redraws without polling.
#[derive(Debug, Clone, Copy)]
pub struct Wake;

#[derive(Clone)]
pub struct Config {
    /// Monospace and Arabic families to try first; `None` uses the built-in
    /// preference lists.
    pub font: Option<String>,
    pub arabic_font: Option<String>,
    pub font_size: f32,
    /// `None` derives the Arabic scale from font metrics.
    pub arabic_scale: Option<f32>,
    pub line_height: f32,
    /// Insets in logical pixels, scaled with the display's backing scale.
    pub padding_x: f32,
    pub padding_y: f32,
    pub bidi_mode: BidiMode,
    pub copy_on_select: bool,
    pub ligatures: bool,
    pub minimum_contrast: f32,
    pub working_directory: Option<String>,
    /// Reapplied after reading the file on an explicit settings reload.
    pub cli_overrides: crate::config::File,
    pub startup_warning: Option<String>,
    pub scrollback: usize,
    pub cols: usize,
    pub rows: usize,
    /// Text written to the shell once it starts — handy for demos and for
    /// driving the terminal from a test.
    pub startup_cmd: Option<String>,
    /// How far a wheel notch or a trackpad gesture scrolls.
    pub wheel: crate::scroll::WheelTuning,
    /// Translate the wheel into arrow keys for full-screen programs that have
    /// not asked for mouse events. Useful for `less` and `man`; unwanted for
    /// anything that binds the arrows to something destructive.
    pub alternate_scroll: bool,
    /// Cursor look when the application has not asked for one.
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// Flash the window edge on BEL.
    pub visual_bell: bool,
    pub palette: crate::color::Palette,
    /// The theme the palette comes from, and the configuration file's colours
    /// laid over it.
    pub theme: &'static Theme,
    pub colors: crate::color::Overrides,
    /// The configuration file, where a theme chosen in the picker is
    /// remembered. `None` keeps the choice for this run only.
    pub config_path: Option<std::path::PathBuf>,
    /// Draw on the CPU even when a GPU is available.
    pub software: bool,
}

/// Where frames go: the GPU compositor, or the software canvas presented
/// through softbuffer when there is no usable GPU or the user asked for it.
enum Output {
    Gpu(Gpu),
    Software(softbuffer::Surface<Arc<Window>, Arc<Window>>),
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: None,
            arabic_font: None,
            font_size: 15.0,
            arabic_scale: None,
            line_height: 1.0,
            padding_x: 12.0,
            padding_y: 10.0,
            bidi_mode: BidiMode::default(),
            copy_on_select: false,
            ligatures: true,
            minimum_contrast: 4.5,
            working_directory: None,
            cli_overrides: Default::default(),
            startup_warning: None,
            scrollback: 10_000,
            cols: 96,
            rows: 28,
            startup_cmd: None,
            wheel: Default::default(),
            alternate_scroll: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            visual_bell: true,
            palette: crate::color::Palette::dark(),
            theme: crate::color::default_theme(),
            colors: Default::default(),
            config_path: None,
            software: std::env::var_os("ALEF_TERM_SOFTWARE").is_some(),
        }
    }
}

impl Config {
    /// `t`'s colours with the configuration file's overrides on top.
    pub fn palette_for(&self, t: &Theme) -> crate::color::Palette {
        let mut p = crate::color::Palette::from_theme(t);
        self.colors.apply(&mut p);
        p
    }

    pub fn set_theme(&mut self, t: &'static Theme) {
        self.theme = t;
        self.palette = self.palette_for(t);
    }
}

/// The window decoration that suits a palette: macOS draws the title bar in
/// the system appearance otherwise, a dark bar over a light theme.
fn window_theme(p: &crate::color::Palette) -> winit::window::Theme {
    if p.is_dark() {
        winit::window::Theme::Dark
    } else {
        winit::window::Theme::Light
    }
}

/// Something a pane asks the application to do on its behalf.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Request {
    NewWindow,
    NewTab,
    Close,
    Quit,
    /// Switch every window to the named theme, and remember it.
    SetTheme(&'static str),
    SetArabicFont(crate::font::ArabicFont),
    Reload,
    /// Show the window's tab at this index. On Linux, where the application
    /// draws tabs itself, the tabs of a window share it.
    ShowTab(usize),
}

/// One window: its terminal, shell, fonts and everything drawn in it.
pub struct Pane {
    cfg: Config,
    base_font_size: f32,
    tab_group: String,
    session_title: String,
    occluded: bool,
    mouse_bypass: bool,
    commands: Option<CommandPalette>,
    notice: Option<(String, std::time::Instant)>,
    /// Directory the shell starts in, for windows opened from another.
    start_dir: Option<String>,
    request: Option<Request>,
    window: Option<Arc<Window>>,
    output: Option<Output>,
    /// Font generation the GPU glyph atlas was built for.
    atlas_generation: u64,
    term: Terminal,
    vt: Vt,
    pty: Option<Pty>,
    fonts: Fonts,
    renderer: Renderer,
    proxy: EventLoopProxy<Wake>,
    modifiers: ModifiersState,
    scale: f32,
    focused: bool,
    selection: Option<Selection>,
    selecting: bool,
    mouse_at: Hit,
    /// Button currently held, for drag reporting.
    mouse_held: Option<u8>,
    /// Consecutive clicks on the same cell: 1, 2 or 3, and when the last was.
    clicks: u8,
    last_click: Option<(std::time::Instant, Hit)>,
    /// The link under the pointer while Cmd is held, and its target.
    hover_link: Option<(Span, String)>,
    /// The search bar, while open, and the index behind it.
    search: Option<Search>,
    search_index: Index,
    /// The theme picker, while open.
    picker: Option<ThemePicker>,
    /// Bumped whenever the terminal's content changes, so the search index
    /// knows when it is stale.
    epoch: u64,
    clipboard: Option<arboard::Clipboard>,
    needs_redraw: bool,
    exited: bool,
    /// Withholds frames while an application is mid-update (mode 2026).
    frame_hold: crate::frame_hold::FrameHold,
    scroll: crate::scroll::Scroller,
    /// Fractional wheel movement not yet worth a whole line. Trackpads deliver
    /// deltas far smaller than a row; without carrying the remainder, either
    /// nothing ever fires or every tiny event fires a full line.
    wheel_remainder: f32,
    last_frame: Option<std::time::Instant>,
    scrollbar_fade: f32,
    frames: FramePacer,
    pty_pending: bool,
    refresh_millihertz: Option<u32>,
    /// Frame timing, enabled with `ALEF_TERM_PROFILE=1`.
    profile: Option<Profile>,
    /// Cursor blink: the current phase and when it next flips. Typing resets
    /// the phase to visible, so the cursor never vanishes under the fingers.
    blink_on: bool,
    blink_at: std::time::Instant,
    /// When the last bell rang, for the flash.
    bell_at: Option<std::time::Instant>,
    /// When the last desktop notification went out, for rate limiting.
    notified_at: Option<std::time::Instant>,
    /// Titles of the tabs in this window and which one is on show, while it
    /// has more than one; set by the application.
    tabs: Option<(Vec<String>, usize)>,
    /// Where the pointer is, in physical pixels.
    pointer: PhysicalPosition<f64>,
}

/// Clicks closer together than this on the same cell count as one gesture.
const MULTI_CLICK: std::time::Duration = std::time::Duration::from_millis(500);

/// Hand a URL to the system's opener.
fn open_url(url: &str) {
    // Only the schemes url_at recognises get here; nothing shell-expanded.
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";
    if let Err(e) = std::process::Command::new(cmd).arg(url).spawn() {
        log::warn!("could not open {url}: {e}");
    }
}

/// The key a chord was pressed on, for matching shortcuts. macOS reports
/// Command chords this way already, Shift aside; X11 and Wayland report what
/// the modifiers produce (Ctrl+Shift+1 is `!`), so ask for the bare key.
fn unmodified_key(event: &winit::event::KeyEvent) -> Key {
    #[cfg(target_os = "macos")]
    return event.logical_key.clone();
    #[cfg(not(target_os = "macos"))]
    {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        event.key_without_modifiers()
    }
}

/// Half a blink period.
const BLINK_HALF: std::time::Duration = std::time::Duration::from_millis(530);
/// How long the bell flash lasts.
const BELL_FLASH: std::time::Duration = std::time::Duration::from_millis(150);

/// Per-phase frame times, reported to stderr every 60 frames.
struct Profile {
    frames: u32,
    sum: [std::time::Duration; 3],
    max: [std::time::Duration; 3],
    since: std::time::Instant,
}

impl Profile {
    fn new() -> Profile {
        Profile {
            frames: 0,
            sum: Default::default(),
            max: Default::default(),
            since: std::time::Instant::now(),
        }
    }

    fn record(&mut self, phases: [std::time::Duration; 3]) {
        self.frames += 1;
        for (i, d) in phases.iter().enumerate() {
            self.sum[i] += *d;
            self.max[i] = self.max[i].max(*d);
        }
        if self.frames == 60 {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let total: std::time::Duration = self.sum.iter().sum();
            let n = self.frames as f64;
            eprintln!(
                "frames: {} in {:.0} ms; avg {:.2} ms/frame  \
                 [scroll {:.2}/{:.2}  draw {:.2}/{:.2}  present {:.2}/{:.2}] (avg/max ms)",
                self.frames,
                ms(self.since.elapsed()),
                ms(total) / n,
                ms(self.sum[0]) / n, ms(self.max[0]),
                ms(self.sum[1]) / n, ms(self.max[1]),
                ms(self.sum[2]) / n, ms(self.max[2]),
            );
            *self = Profile::new();
        }
    }
}

impl Pane {
    pub fn new(cfg: Config, proxy: EventLoopProxy<Wake>, start_dir: Option<String>) -> anyhow::Result<Pane> {
        let mut fonts = Fonts::with_families(
            cfg.font_size,
            cfg.arabic_scale,
            cfg.font.as_deref(),
            cfg.arabic_font.as_deref(),
        )?;
        fonts.set_line_height(cfg.line_height);
        fonts.set_ligatures(cfg.ligatures);
        let mut renderer = Renderer::new();
        renderer.pad_x = cfg.padding_x;
        renderer.pad_y = cfg.padding_y;
        renderer.minimum_contrast = cfg.minimum_contrast;
        let mut term = Terminal::new(cfg.rows, cfg.cols, cfg.scrollback);
        term.set_palette(cfg.palette.clone());
        term.bidi_mode = cfg.bidi_mode;
        Ok(Pane {
            base_font_size: cfg.font_size,
            tab_group: String::new(),
            occluded: false,
            mouse_bypass: false,
            commands: None,
            notice: cfg.startup_warning.as_ref().map(|text| (text.clone(), std::time::Instant::now() + std::time::Duration::from_secs(10))),
            session_title: "alef-term".into(),
            cfg,
            start_dir,
            request: None,
            window: None,
            output: None,
            atlas_generation: 0,
            term,
            vt: Vt::new(),
            pty: None,
            fonts,
            renderer,
            proxy,
            modifiers: ModifiersState::empty(),
            scale: 1.0,
            focused: true,
            selection: None,
            selecting: false,
            mouse_at: Hit::default(),
            mouse_held: None,
            clicks: 0,
            last_click: None,
            hover_link: None,
            search: None,
            search_index: Index::default(),
            picker: None,
            epoch: 0,
            clipboard: arboard::Clipboard::new().map_err(|error| {
                log::warn!("could not initialize clipboard: {error}");
            }).ok(),
            needs_redraw: true,
            exited: false,
            frame_hold: crate::frame_hold::FrameHold::new(),
            scroll: Default::default(),
            wheel_remainder: 0.0,
            last_frame: None,
            scrollbar_fade: 0.0,
            profile: std::env::var_os("ALEF_TERM_PROFILE").map(|_| Profile::new()),
            blink_on: true,
            blink_at: std::time::Instant::now() + BLINK_HALF,
            bell_at: None,
            notified_at: None,
            tabs: None,
            pointer: PhysicalPosition::new(0.0, 0.0),
            frames: FramePacer::default(),
            pty_pending: false,
            refresh_millihertz: None,
        })
    }

    fn cell_size(&self) -> (f32, f32) {
        (self.fonts.cell_w, self.fonts.cell_h)
    }

    /// Whether the cursor is blinking right now: the application or the
    /// configuration asked for it, the window is focused, and the view is on
    /// the live screen where the cursor is drawn at all.
    fn blinking(&self) -> bool {
        self.term.cursor_style.blink.unwrap_or(self.cfg.cursor_blink)
            && self.focused && !self.occluded
            && self.term.modes.contains(Modes::CURSOR_VISIBLE)
            && self.term.grid.view_offset == 0
    }

    /// Scroll to the previous (`-1`) or next (`+1`) prompt the shell marked
    /// with OSC 133, putting it at the top of the view. Returns false when
    /// there is none to go to, so the key can fall through.
    fn jump_prompt(&mut self, dir: i32) -> bool {
        let prompts = self.term.prompt_lines().to_vec();
        let grid = &self.term.grid;
        let base = grid.scrolled as i64 - grid.scrollback.len() as i64;
        // The absolute line at the top of the view now.
        let top = base + grid.viewport_top() as i64;
        let target = if dir < 0 {
            prompts.iter().rev().find(|&&l| (l as i64) < top).copied()
        } else {
            prompts.iter().find(|&&l| (l as i64) > top).copied()
        };
        let Some(line) = target else {
            if dir > 0 && !self.scroll.at_bottom() {
                self.scroll_to(0.0);
                return true;
            }
            return false;
        };
        // Offset back from the live screen that puts `line` at the top.
        let off = (grid.scrolled as i64 - line as i64).max(0) as f32;
        let max = self.scroll_max();
        self.scroll.jump_to(off, max);
        self.needs_redraw = true;
        true
    }

    /// Open the theme picker on the theme in use.
    fn picker_open(&mut self) {
        self.commands = None;
        self.search = None;
        self.picker = Some(ThemePicker::new(self.cfg.theme));
        self.needs_redraw = true;
    }

    /// Show the highlighted theme in this window without committing to it.
    fn picker_preview(&mut self) {
        let Some(t) = self.picker.as_ref().and_then(|p| p.current()) else { return };
        self.show_palette(self.cfg.palette_for(t));
        self.needs_redraw = true;
    }

    /// Close the picker, back on the theme in use before it opened.
    fn picker_cancel(&mut self) {
        if self.picker.take().is_some() {
            self.show_palette(self.cfg.palette.clone());
        }
    }

    /// Keys while the picker is open. Returns true when consumed.
    fn picker_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let Some(picker) = &mut self.picker else { return false };
        let command = PLATFORM.command(self.modifiers);
        let erase_all = PLATFORM.erase_all(self.modifiers);
        // The key that opened it closes it.
        let closes = PLATFORM.shortcut(key, self.modifiers) == Some(Shortcut::Do(Action::Theme));
        let shift = self.modifiers.shift_key();
        match key {
            Key::Named(NamedKey::Escape) => {
                self.picker_cancel();
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                // With nothing matching, the picker stays open.
                if let Some(t) = picker.current() {
                    self.picker = None;
                    self.request = Some(Request::SetTheme(t.name));
                    self.needs_redraw = true;
                }
                return true;
            }
            Key::Named(NamedKey::ArrowUp) => picker.step(-1),
            Key::Named(NamedKey::ArrowDown) => picker.step(1),
            Key::Named(NamedKey::Tab) => picker.step(if shift { -1 } else { 1 }),
            Key::Named(NamedKey::Backspace) => picker.pop(erase_all),
            _ if closes => {
                self.picker_cancel();
                return true;
            }
            _ if command => return false,
            _ => {
                let Some(t) = text else { return true };
                let t: String = t.chars().filter(|c| !c.is_control()).collect();
                if t.is_empty() {
                    return true;
                }
                picker.push(&t);
            }
        }
        self.picker_preview();
        true
    }

    fn apply_arabic_font(&mut self, family: &str, mut fonts: Fonts) {
        self.cfg.arabic_font = Some(family.to_string());
        fonts.set_line_height(self.cfg.line_height);
        fonts.set_ligatures(self.cfg.ligatures);
        self.fonts = fonts;
        self.renderer.invalidate_layouts();
        self.atlas_generation = u64::MAX;
        self.selection = None;
        self.search = None;
        self.reflow();
        self.notify(format!("Arabic font: {family}"));
    }

    /// Switch to `t` for good: this window's colours, and what it restores
    /// when a program resets them.
    fn apply_theme(&mut self, t: &'static Theme) {
        self.cfg.set_theme(t);
        self.show_palette(self.cfg.palette.clone());
    }

    fn apply_config(&mut self, cfg: &Config, fonts: Option<Fonts>) {
        self.picker = None;
        self.commands = None;
        self.selection = None;
        self.search = None;
        if let Some(fonts) = fonts { self.fonts = fonts; }
        self.fonts.set_line_height(cfg.line_height);
        self.fonts.set_ligatures(cfg.ligatures);
        self.base_font_size = cfg.font_size;
        let renderer_changed = self.cfg.software != cfg.software;
        self.cfg = cfg.clone();
        self.set_display_scale(self.scale);
        self.renderer.invalidate_layouts();
        self.renderer.minimum_contrast = cfg.minimum_contrast;
        self.atlas_generation = u64::MAX;
        self.term.bidi_mode = cfg.bidi_mode;
        self.term.grid.max_scrollback = cfg.scrollback;
        let excess = self.term.grid.scrollback.len().saturating_sub(cfg.scrollback);
        self.term.grid.scrollback.drain(..excess);
        self.scroll_to(self.scroll.pos.min(self.scroll_max()));
        self.epoch += 1;
        self.show_palette(cfg.palette.clone());
        self.reflow();
        self.notify(if renderer_changed {
            "Settings reloaded; renderer changes apply to new windows"
        } else { "Settings reloaded" });
    }

    /// Draw with `p` from the next frame, title bar included.
    fn show_palette(&mut self, p: crate::color::Palette) {
        if let Some(w) = &self.window {
            w.set_theme(Some(window_theme(&p)));
        }
        self.term.set_palette(p);
        self.needs_redraw = true;
        // A program that asked (mode 2031) hears about the change now, not
        // whenever the shell next writes something.
        if !self.term.replies.is_empty() {
            let replies = std::mem::take(&mut self.term.replies);
            self.write_pty(&replies);
        }
    }

    /// Open the search bar, seeded with a one-line selection if there is one.
    fn search_open(&mut self) {
        self.commands = None;
        self.picker_cancel();
        let seed = self
            .selection
            .map(|s| selected_text(&self.term, s))
            .filter(|t| !t.is_empty() && !t.contains('\n'));
        let mut search = self.search.take().unwrap_or_default();
        if let Some(q) = seed {
            search.query = q;
        }
        self.search = Some(search);
        self.search_run(true);
    }

    /// Re-run the query. With `jump`, the view moves to the hit nearest the
    /// bottom of what is showing; otherwise it stays put and the current hit
    /// is kept by line.
    fn search_run(&mut self, jump: bool) {
        self.search_index.update(&self.term.grid, self.epoch);
        let Some(search) = &mut self.search else { return };
        let near = if jump {
            let bottom = self.term.grid.viewport_top() + self.term.grid.rows as isize - 1;
            Some(bottom.max(0) as usize)
        } else {
            search.current_line()
        };
        search.run(&self.search_index, near);
        if jump {
            self.search_show_current();
        }
        self.needs_redraw = true;
    }

    /// Scroll so the current hit sits mid-screen.
    fn search_show_current(&mut self) {
        let Some(line) = self.search.as_ref().and_then(|s| s.current_line()) else { return };
        let sb = self.term.grid.scrollback.len() as isize;
        let off = (sb - line as isize + self.term.grid.rows as isize / 2).max(0) as f32;
        let max = self.scroll_max();
        self.scroll.jump_to(off, max);
        self.needs_redraw = true;
    }

    /// Keys while the search bar is open. Returns true when consumed.
    fn search_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let Some(search) = &mut self.search else { return false };
        let command = PLATFORM.command(self.modifiers);
        let shift = self.modifiers.shift_key();
        match key {
            Key::Named(NamedKey::Escape) => {
                self.search = None;
                self.needs_redraw = true;
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown) => {
                let newer = matches!(key, Key::Named(NamedKey::ArrowDown)) || (matches!(key, Key::Named(NamedKey::Enter)) && shift);
                search.step(if newer { 1 } else { -1 });
                self.search_show_current();
            }
            Key::Named(NamedKey::Backspace) => {
                if PLATFORM.erase_all(self.modifiers) {
                    search.query.clear();
                } else {
                    search.query.pop();
                }
                self.search_run(true);
            }
            Key::Character(c) if command && c.as_str() == "g" => {
                search.step(if shift { 1 } else { -1 });
                self.search_show_current();
            }
            Key::Character(c) if command && c.as_str() == "v" => {
                self.paste_clipboard();
            }
            Key::Character(c) if command && c.as_str() == "f" => {
                // Cmd-F again with a selection re-seeds the query.
                self.search_open();
            }
            _ if command => return false,
            _ => {
                let Some(t) = text else { return true };
                let t: String = t.chars().filter(|c| !c.is_control()).collect();
                if !t.is_empty() {
                    search.query.push_str(&t);
                    self.search_run(true);
                }
            }
        }
        true
    }

    /// Underline the link under the pointer while Cmd (Ctrl on Linux) is held.
    fn update_hover(&mut self) {
        let at = self.mouse_at;
        let link = if PLATFORM.link(self.modifiers) && !self.mouse_grabbed() {
            url_at(&self.term, at.line, at.cell)
        } else {
            None
        };
        if link.as_ref().map(|l| l.0) != self.hover_link.as_ref().map(|l| l.0) {
            self.needs_redraw = true;
        }
        self.hover_link = link;
    }

    /// Show the cursor and restart the blink period, as on every keystroke.
    fn blink_reset(&mut self) {
        if !self.blink_on {
            self.needs_redraw = true;
        }
        self.blink_on = true;
        self.blink_at = std::time::Instant::now() + BLINK_HALF;
    }

    /// Recompute the grid from the window size and tell the child about it.
    fn reflow(&mut self) {
        let Some(win) = &self.window else { return };
        self.refresh_millihertz = win.current_monitor().and_then(|m| m.refresh_rate_millihertz());
        let size = win.inner_size();
        let (cw, ch) = self.cell_size();
        let (px, py) = (self.renderer.pad_x, self.renderer.pad_y);
        let cols = (((size.width as f32 - 2.0 * px) / cw).floor() as usize).max(1);
        let rows = (((size.height as f32 - 2.0 * py - self.tab_bar_height()) / ch).floor() as usize).max(1);
        if cols != self.term.grid.cols || rows != self.term.grid.rows {
            if cols != self.term.grid.cols {
                // Lines move when the width changes; a selection anchored to
                // the old ones would land on the wrong text.
                self.selection = None;
            }
            self.term.resize(rows, cols);
            self.epoch += 1;
        }
        self.term.set_cell_px(cw, ch);
        self.term.prune_images();
        if let Some(pty) = &self.pty {
            pty.resize(rows as u16, cols as u16, cw as u16, ch as u16);
        }
        self.needs_redraw = true;
    }

    fn drain_pty(&mut self) {
        let Some(pty) = &self.pty else { return };
        let was_at_bottom = self.scroll.at_bottom();
        let retired_before = self.term.grid.scrolled;
        let result = pty.drain(|bytes| self.vt.feed(&mut self.term, bytes));
        self.pty_pending = result.yielded;
        if result.bytes > 0 {
            self.epoch += 1;
            let retired = (self.term.grid.scrolled - retired_before) as f32;

            if was_at_bottom {
                // Following the tail: stay pinned to the live screen.
                self.scroll.snap_to_bottom();
                self.term.grid.view_offset = 0;
                self.renderer.scroll_frac = 0.0;
            } else if retired > 0.0 {
                // Scrolled back. Output must not drag the view along — a
                // program that repaints continuously would otherwise make
                // scrolling impossible, cancelling it within milliseconds.
                // Follow the retiring lines so the same text stays on screen.
                let max = self.scroll_max();
                self.scroll.shift(retired, max);
                self.term.grid.view_offset = self.scroll.offset();
                self.needs_redraw = true;
            }
        }
        let bells = std::mem::take(&mut self.term.bells);
        if bells > 0 && self.cfg.visual_bell {
            self.bell_at = Some(std::time::Instant::now());
            self.needs_redraw = true;
        }
        if bells > 0 && !self.focused {
            self.request_attention();
        }
        for n in std::mem::take(&mut self.term.notifications) {
            self.show_notification(n);
        }
        if let Some(text) = self.term.clipboard_write.take() {
            if let Some(cb) = &mut self.clipboard {
                let _ = cb.set_text(text);
            }
        }
        if !self.term.replies.is_empty() {
            let replies = std::mem::take(&mut self.term.replies);
            if let Some(p) = &mut self.pty {
                p.write(&replies);
            }
        }
        // Synchronized output. The mode is read *after* parsing the batch: a
        // program often brackets an entire repaint inside one write, and in
        // that case the update is already complete and nothing should be held.
        let damaged = std::mem::take(&mut self.term.damaged);
        let syncing = self.term.modes.contains(Modes::SYNC_OUTPUT);
        let out = self.frame_hold.poll(syncing, damaged, std::time::Instant::now());
        if out.drop_mode {
            // The hold overran its watchdog, so the program that opened it is
            // not going to close it. Stop honouring the mode rather than
            // freezing the display.
            log::debug!("synchronized output watchdog fired; releasing the frame");
            self.term.modes.remove(Modes::SYNC_OUTPUT);
        }
        if out.redraw {
            self.needs_redraw = true;
        }
        if result.exited {
            self.exited = true;
        }
    }

    /// Advance the scroll animation by one frame, and mirror the result into
    /// the grid and renderer. Returns true while anything is still moving.
    fn step_scroll(&mut self) -> bool {
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .clamp(0.0, 0.1);
        self.last_frame = Some(now);

        let max = self.scroll_max();
        let moving = self.scroll.step(dt, max);

        if self.term.grid.view_offset != self.scroll.offset() {
            self.term.grid.view_offset = self.scroll.offset();
            self.needs_redraw = true;
        }
        if (self.renderer.scroll_frac - self.scroll.frac()).abs() > 0.001 {
            self.renderer.scroll_frac = self.scroll.frac();
            self.needs_redraw = true;
        }

        // The scrollbar shows while scrolled or moving, then fades away.
        let want = if moving || !self.scroll.at_bottom() { 1.0 } else { 0.0 };
        let fading = (self.scrollbar_fade - want).abs() > 0.01;
        if fading {
            let k = 1.0 - (0.02f32).powf(dt);
            self.scrollbar_fade += (want - self.scrollbar_fade) * k;
            self.needs_redraw = true;
        } else {
            self.scrollbar_fade = want;
        }
        self.renderer.scrollbar_alpha = self.scrollbar_fade;

        moving || fading
    }

    /// Apply a wheel movement to our own scrollback. Accumulate only; the
    /// frame applies it, so a burst of trackpad events costs one redraw.
    fn scroll_wheel(&mut self, lines: f32) {
        self.scroll.nudge(lines);
        self.needs_redraw = true;
    }

    /// How far back the view may go. The alternate screen keeps no history of
    /// its own, and showing the primary screen's scrollback above alt-screen
    /// rows would splice two unrelated buffers together — so while a
    /// full-screen program is up, there is nowhere to scroll.
    #[inline]
    fn scroll_max(&self) -> f32 {
        if self.term.grid.in_alt() {
            0.0
        } else {
            self.term.grid.scrollback.len() as f32
        }
    }

    /// Scroll by a number of lines. Positive moves back into history.
    fn scroll_by(&mut self, lines: f32) {
        self.scroll.nudge(lines);
        self.needs_redraw = true;
    }

    fn scroll_to(&mut self, lines: f32) {
        let max = self.scroll_max();
        self.scroll.jump_to(lines, max);
        self.needs_redraw = true;
    }

    /// Write bytes the terminal generates on the application's behalf — mouse
    /// reports, focus notifications. These must not disturb the view: reporting
    /// a wheel tick that the application is about to act on should not also
    /// yank the screen to the bottom.
    fn write_pty(&mut self, bytes: &[u8]) {
        if let Some(p) = &mut self.pty {
            p.write(bytes);
        }
    }

    /// Write bytes the user typed. Typing returns to the live screen, as every
    /// terminal does.
    fn send(&mut self, bytes: &[u8]) {
        self.write_pty(bytes);
        if !self.scroll.at_bottom() {
            self.scroll.snap_to_bottom();
            self.term.grid.view_offset = 0;
            self.renderer.scroll_frac = 0.0;
            self.needs_redraw = true;
        }
    }

    fn key_bytes(&self, key: &Key, text: Option<&str>) -> Option<Vec<u8>> {
        crate::keys::encode(
            key,
            text,
            self.modifiers,
            crate::keys::KeyContext {
                app_cursor: self.term.modes.contains(Modes::APP_CURSOR),
                kitty_flags: self.term.kitty_flags(),
            },
        )
    }

    /// Where a pixel position lands, in both coordinate spaces.
    ///
    /// The two must not be confused. Selection needs the **absolute line**, so
    /// it stays on its text as the view scrolls. Mouse reports need the
    /// **viewport row**, because that is the only thing the application knows
    /// about — it has no idea our scrollback exists.
    fn hit_test(&self, pos: PhysicalPosition<f64>) -> Hit {
        let (cw, ch) = self.cell_size();
        let (px, py) = (self.renderer.pad_x, self.renderer.pad_y + self.tab_bar_height());
        let row = (((pos.y as f32 - py) / ch).floor().max(0.0) as usize)
            .min(self.term.grid.rows.saturating_sub(1));
        let col = (((pos.x as f32 - px) / cw).floor().max(0.0) as usize)
            .min(self.term.grid.cols.saturating_sub(1));
        let cell = match self.renderer.row_layout(row) {
            Some(l) => l.cell_at(col),
            None => col,
        };
        Hit {
            row,
            line: (self.term.grid.viewport_top() + row as isize).max(0) as usize,
            cell,
        }
    }

    fn tab_bar_height(&self) -> f32 {
        if self.tabs.is_some() { crate::render::tab_bar_height(&self.fonts) } else { 0.0 }
    }

    /// The title shown for this session: the program's, or the session name.
    fn tab_title(&self) -> String {
        if self.term.title.is_empty() || self.term.title == "alef-term" {
            self.session_title.clone()
        } else {
            self.term.title.clone()
        }
    }

    fn set_tabs(&mut self, tabs: Option<(Vec<String>, usize)>) {
        if tabs == self.tabs {
            return;
        }
        let bar_changed = tabs.is_some() != self.tabs.is_some();
        self.tabs = tabs;
        if self.output.is_some() {
            if bar_changed {
                self.reflow();
            }
            self.needs_redraw = true;
        }
    }

    /// The tab under `pos`, when it is on the tab bar.
    fn tab_at(&self, pos: PhysicalPosition<f64>) -> Option<usize> {
        let (titles, _) = self.tabs.as_ref()?;
        if pos.y as f32 >= self.tab_bar_height() {
            return None;
        }
        let width = self.window.as_ref()?.inner_size().width.max(1) as f64;
        Some(((pos.x.max(0.0) / width * titles.len() as f64) as usize).min(titles.len() - 1))
    }

    /// Ask for the tab `step` along from this one, wrapping at the ends.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn step_tab(&mut self, step: isize) {
        if let Some((titles, active)) = &self.tabs {
            let index = (*active as isize + step).rem_euclid(titles.len() as isize) as usize;
            self.request = Some(Request::ShowTab(index));
        }
    }

    /// Ask for tab `number`, counting from 1; 9 is always the last.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn select_tab(&mut self, number: usize) {
        if let Some((titles, _)) = &self.tabs {
            let index = if number == 9 { titles.len() - 1 } else { number - 1 };
            if index < titles.len() {
                self.request = Some(Request::ShowTab(index));
            }
        }
    }

    /// Focus arriving or leaving, from the window or from switching tabs,
    /// reported to programs that asked for it.
    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        if self.term.modes.contains(Modes::FOCUS_EVENTS) {
            self.write_pty(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
        self.needs_redraw = true;
    }

    /// Middle-click paste: the primary selection on X11 and Wayland, the
    /// clipboard on macOS, which has no primary selection.
    fn paste_primary(&mut self) {
        #[cfg(target_os = "macos")]
        self.paste_clipboard();
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let Some(clipboard) = &mut self.clipboard else { return };
            match crate::clipboard::primary_text(clipboard) {
                Ok(Some(text)) => self.paste(&text),
                Ok(None) => {}
                Err(error) => log::warn!("primary selection paste failed: {error:#}"),
            }
        }
    }

    fn paste_clipboard(&mut self) {
        let allow_paths = self.commands.is_none() && self.picker.is_none() && self.search.is_none();
        let Some(clipboard) = &mut self.clipboard else {
            self.notify("Clipboard is unavailable");
            return;
        };
        match crate::clipboard::paste_text(clipboard, allow_paths) {
            Ok(Some(text)) => self.paste(&text),
            Ok(None) => self.notify(if allow_paths {
                "No text, files, or image is available on the clipboard"
            } else {
                "No text is available on the clipboard"
            }),
            Err(error) => {
                log::warn!("clipboard paste failed: {error:#}");
                self.notify(error.to_string());
            }
        }
    }

    fn paste(&mut self, text: &str) {
        if let Some(commands) = &mut self.commands {
            commands.push(text);
            self.needs_redraw = true;
        } else if let Some(picker) = &mut self.picker {
            picker.push(&text.chars().filter(|c| !c.is_control()).collect::<String>());
            self.picker_preview();
        } else if let Some(search) = &mut self.search {
            search.query.extend(text.chars().filter(|c| !c.is_control()));
            self.search_run(true);
        } else {
            let bytes = crate::input::paste_bytes(text, self.term.modes.contains(Modes::BRACKETED_PASTE));
            if !bytes.is_empty() { self.send(&bytes); }
        }
    }

    /// Show a notification a program asked for. In a window the user is
    /// looking at, a notice is enough; otherwise it goes to the desktop, at
    /// most one a second so a runaway program cannot flood it, and the window
    /// asks for attention.
    fn show_notification(&mut self, n: crate::term::Notification) {
        let title = if !n.title.is_empty() {
            n.title
        } else if self.term.title != "alef-term" && !self.term.title.is_empty() {
            self.term.title.clone()
        } else {
            "Alef".to_string()
        };
        if self.focused {
            let text = if n.body.is_empty() { title } else { format!("{title}: {}", n.body) };
            self.notify(text.replace('\n', " "));
            return;
        }
        let now = std::time::Instant::now();
        if self.notified_at.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(1)) {
            log::debug!("dropping a notification: more than one a second");
            return;
        }
        self.notified_at = Some(now);
        crate::notify::show(&title, &n.body);
        self.request_attention();
    }

    /// Mark the window as wanting attention: the dock bounces on macOS, and
    /// the task bar highlights it on Linux. Focusing the window clears it.
    fn request_attention(&self) {
        if let Some(window) = &self.window {
            window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), std::time::Instant::now() + std::time::Duration::from_secs(4)));
        self.needs_redraw = true;
    }

    fn command_open(&mut self) {
        self.picker_cancel();
        self.search = None;
        self.commands = if self.commands.is_some() { None } else { Some(CommandPalette::default()) };
        self.needs_redraw = true;
    }

    fn command_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let Some(commands) = &mut self.commands else { return false };
        match key {
            Key::Named(NamedKey::Escape) => self.commands = None,
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = commands.current() {
                    self.commands = None;
                    self.perform_action(action);
                }
            }
            Key::Named(NamedKey::ArrowUp) => commands.step(-1),
            Key::Named(NamedKey::ArrowDown) => commands.step(1),
            Key::Named(NamedKey::Tab) => commands.step(if self.modifiers.shift_key() { -1 } else { 1 }),
            Key::Named(NamedKey::Backspace) => commands.backspace(PLATFORM.erase_all(self.modifiers)),
            _ if PLATFORM.command(self.modifiers) => return false,
            _ => if let Some(text) = text { commands.push(text); },
        }
        self.needs_redraw = true;
        true
    }

    fn run_shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::Do(action) => self.perform_action(action),
            Shortcut::Palette => self.command_open(),
            Shortcut::CycleDirection => self.perform_action(match self.term.bidi_mode {
                BidiMode::Auto => Action::DirectionLtr,
                BidiMode::ForceLtr => Action::DirectionRtl,
                BidiMode::ForceRtl => Action::DirectionAuto,
            }),
            Shortcut::SelectTab(number) => {
                #[cfg(target_os = "macos")]
                if let Some(window) = &self.window {
                    use winit::platform::macos::WindowExtMacOS;
                    window.select_tab_at_index(if number == 9 { window.num_tabs().saturating_sub(1) } else { number - 1 });
                }
                #[cfg(not(target_os = "macos"))]
                self.select_tab(number);
            }
            Shortcut::Close => self.request = Some(Request::Close),
            Shortcut::Quit => self.request = Some(Request::Quit),
        }
    }

    fn perform_action(&mut self, action: Action) {
        match action {
            Action::NewTab => self.request = Some(Request::NewTab),
            Action::NextTab | Action::PreviousTab => {
                #[cfg(target_os = "macos")]
                if let Some(window) = &self.window {
                    use winit::platform::macos::WindowExtMacOS;
                    if action == Action::NextTab { window.select_next_tab(); }
                    else { window.select_previous_tab(); }
                }
                #[cfg(not(target_os = "macos"))]
                self.step_tab(if action == Action::NextTab { 1 } else { -1 });
            }
            Action::NewWindow => self.request = Some(Request::NewWindow),
            Action::Search => self.search_open(),
            Action::Theme => self.picker_open(),
            Action::ArabicFonts => {
                self.picker_cancel();
                self.search = None;
                let mut commands = CommandPalette::default();
                commands.push("Arabic font");
                self.commands = Some(commands);
            }
            Action::ArabicFont(font) => self.request = Some(Request::SetArabicFont(font)),
            Action::Reload => self.request = Some(Request::Reload),
            Action::Copy => {
                let text = self.selection.map(|s| selected_text(&self.term, s)).unwrap_or_default();
                if !text.is_empty() {
                    if self.clipboard.as_mut().is_none_or(|cb| cb.set_text(text).is_err()) {
                        self.notify("Could not copy to the clipboard");
                    }
                }
            }
            Action::Paste => self.paste_clipboard(),
            Action::SelectAll => {
                let start = if self.term.grid.in_alt() { self.term.grid.scrollback.len() } else { 0 };
                let end = self.term.grid.scrollback.len() + self.term.grid.rows - 1;
                self.selection = Some(Selection { start: (start, 0), end: (end, self.term.grid.cols - 1), block: false });
            }
            Action::ClearHistory => {
                self.term.grid.scrollback.clear();
                self.selection = None;
                self.search = None;
                self.scroll_to(0.0);
                self.term.prune_images();
                self.epoch += 1;
                self.notify("Scrollback cleared");
            }
            Action::ZoomIn => self.set_font_size(self.cfg.font_size + 1.0),
            Action::ZoomOut => self.set_font_size(self.cfg.font_size - 1.0),
            Action::ZoomReset => self.set_font_size(self.base_font_size),
            Action::DirectionAuto | Action::DirectionLtr | Action::DirectionRtl => {
                let (mode, label) = match action {
                    Action::DirectionAuto => (BidiMode::Auto, "Text direction: automatic"),
                    Action::DirectionLtr => (BidiMode::ForceLtr, "Text direction: left to right"),
                    _ => (BidiMode::ForceRtl, "Text direction: right to left"),
                };
                self.term.bidi_mode = mode;
                self.notify(label);
            }
        }
        self.needs_redraw = true;
    }

    fn set_font_size(&mut self, size: f32) {
        self.cfg.font_size = size.clamp(6.0, 72.0);
        self.fonts.set_size(self.cfg.font_size * self.scale);
        self.reflow();
    }

    fn set_display_scale(&mut self, scale: f32) {
        self.scale = scale;
        self.fonts.set_size(self.cfg.font_size * scale);
        self.renderer.pad_x = (self.cfg.padding_x * scale).round();
        self.renderer.pad_y = (self.cfg.padding_y * scale).round();
    }

    /// True when an application has asked to receive mouse events.
    #[inline]
    fn mouse_grabbed(&self) -> bool {
        self.term
            .modes
            .intersects(Modes::MOUSE_CLICK | Modes::MOUSE_DRAG | Modes::MOUSE_MOTION)
    }

    fn mouse_report(&mut self, button: u8, pressed: bool, row: usize, col: usize) {
        if !self.mouse_grabbed() {
            return;
        }
        // Never report a position inside the scrollback: the application knows
        // nothing about our history and would act on the wrong cell.
        if self.term.grid.view_offset != 0 {
            return;
        }
        let Some(seq) = crate::mouse::encode(
            button,
            pressed,
            row,
            col,
            self.modifiers,
            self.term.modes.contains(Modes::MOUSE_SGR),
        ) else {
            return;
        };
        self.write_pty(&seq);
    }

    /// Report pointer motion, for the drag (1002) and any-motion (1003) modes.
    ///
    /// These were previously accepted as modes but never honoured, so an
    /// application that asked for them saw a dead mouse.
    fn motion_report(&mut self, row: usize, cell: usize) {
        use crate::mouse::{BTN_NONE, MOTION};
        let dragging = self.mouse_held.is_some();
        if self.term.modes.contains(Modes::MOUSE_MOTION) {
            let b = self.mouse_held.unwrap_or(BTN_NONE);
            self.mouse_report(MOTION + b, true, row, cell);
        } else if dragging && self.term.modes.contains(Modes::MOUSE_DRAG) {
            // 1002 reports motion only while a button is down.
            let b = self.mouse_held.unwrap_or(BTN_NONE);
            self.mouse_report(MOTION + b, true, row, cell);
        }
    }
}

impl Pane {
    fn window_id(&self) -> Option<WindowId> {
        self.window.as_ref().map(|w| w.id())
    }

    /// Create the window and start the shell.
    fn open(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (cw, ch) = self.cell_size();
        let attrs = Window::default_attributes()
            .with_title(&self.session_title)
            .with_theme(Some(window_theme(&self.cfg.palette)))
            .with_inner_size(LogicalSize::new(
                self.cfg.cols as f32 * cw + 2.0 * self.cfg.padding_x,
                self.cfg.rows as f32 * ch + 2.0 * self.cfg.padding_y,
            ));
        #[cfg(target_os = "macos")]
        let attrs = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs.with_tabbing_identifier(&self.tab_group)
        };
        // The X11 class and Wayland app id, which the desktop matches against
        // alef-term.desktop for the launcher icon and window grouping.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attrs = {
            use winit::platform::x11::WindowAttributesExtX11;
            attrs.with_name("alef-term", "alef-term")
        };
        let win = Arc::new(el.create_window(attrs).expect("create window"));
        win.set_ime_allowed(true); // needed for Arabic input methods

        self.set_display_scale(win.scale_factor() as f32);
        // Recompute the initial size from the physical font metrics: rounding
        // at Retina scale need not be twice the rounding at logical scale.
        let _ = win.request_inner_size(winit::dpi::PhysicalSize::new(
            (self.cfg.cols as f32 * self.fonts.cell_w + 2.0 * self.renderer.pad_x) as u32,
            (self.cfg.rows as f32 * self.fonts.cell_h + 2.0 * self.renderer.pad_y) as u32,
        ));

        let gpu = if self.cfg.software {
            None
        } else {
            match Gpu::new(win.clone()) {
                Ok(g) => Some(g),
                Err(e) => {
                    eprintln!("alef-term: no GPU ({e:#}); drawing in software");
                    None
                }
            }
        };
        let output = match gpu {
            Some(g) => Output::Gpu(g),
            None => {
                let ctx = softbuffer::Context::new(win.clone()).expect("softbuffer context");
                Output::Software(softbuffer::Surface::new(&ctx, win.clone()).expect("softbuffer surface"))
            }
        };
        self.atlas_generation = self.fonts.generation();
        self.start(win, output);
    }

    /// Draw in `win` through `output` and start the shell. A new window comes
    /// here from `open`; a new tab brings its window and that window's surface.
    fn start(&mut self, win: Arc<Window>, output: Output) {
        self.output = Some(output);
        self.window = Some(win);

        let (cw, ch) = self.cell_size();
        let proxy = self.proxy.clone();
        match Pty::spawn(
            None,
            self.term.grid.rows as u16,
            self.term.grid.cols as u16,
            cw as u16,
            ch as u16,
            self.start_dir.as_deref(),
            move || {
                let _ = proxy.send_event(Wake);
            },
        ) {
            Ok(mut p) => {
                if let Some(cmd) = self.cfg.startup_cmd.clone() {
                    p.write(cmd.as_bytes());
                }
                self.pty = Some(p)
            }
            Err(e) => {
                eprintln!("failed to start shell: {e}");
                self.exited = true;
            }
        }
        self.reflow();
    }

    fn handle(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.request = Some(Request::Close);
            }
            WindowEvent::Occluded(hidden) => {
                self.occluded = hidden;
                if !hidden { self.needs_redraw = true; }
            }
            WindowEvent::Focused(f) => self.set_focus(f),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.set_display_scale(scale_factor as f32);
                self.reflow();
            }
            WindowEvent::Resized(_) => self.reflow(),
            WindowEvent::Moved(_) => {
                self.refresh_millihertz = self.window.as_ref()
                    .and_then(|w| w.current_monitor())
                    .and_then(|m| m.refresh_rate_millihertz());
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                self.update_hover();
            }
            WindowEvent::Ime(winit::event::Ime::Commit(s)) => {
                if let Some(commands) = &mut self.commands {
                    commands.push(&s);
                    self.needs_redraw = true;
                } else if let Some(picker) = &mut self.picker {
                    picker.push(&s);
                    self.picker_preview();
                } else if let Some(search) = &mut self.search {
                    search.query.push_str(&s);
                    self.search_run(true);
                } else {
                    self.send(s.as_bytes());
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                self.blink_reset();
                let key = if PLATFORM.chord(self.modifiers) {
                    crate::input::command_key(&unmodified_key(&event), event.physical_key)
                } else { event.logical_key.clone() };
                if self.commands.is_some() && self.command_key(&key, event.text.as_deref()) { return; }
                if self.picker.is_some() && self.picker_key(&key, event.text.as_deref()) {
                    return;
                }
                if self.search.is_some() && self.search_key(&key, event.text.as_deref()) {
                    return;
                }
                if let Some(shortcut) = PLATFORM.shortcut(&key, self.modifiers) {
                    if !PLATFORM.passes(shortcut, self.tabs.is_some()) {
                        self.run_shortcut(shortcut);
                        return;
                    }
                }
                // Command chords never reach the program on macOS. On Linux a
                // Ctrl+Shift key that is not a shortcut still does, unless an
                // overlay has the keyboard.
                let overlay = self.commands.is_some() || self.picker.is_some() || self.search.is_some();
                if PLATFORM.command(self.modifiers) && (PLATFORM == Platform::Mac || overlay) {
                    return;
                }
                // Shift + navigation keys drive our scrollback rather than
                // being sent on, matching xterm and every terminal since.
                if self.modifiers.shift_key() && !self.term.grid.in_alt() {
                    let page = self.term.grid.rows.saturating_sub(1) as f32;
                    let handled = match &event.logical_key {
                        // Previous / next prompt, when the shell marks them.
                        Key::Named(NamedKey::ArrowUp) => self.jump_prompt(-1),
                        Key::Named(NamedKey::ArrowDown) => self.jump_prompt(1),
                        Key::Named(NamedKey::PageUp) => {
                            self.scroll_by(page);
                            true
                        }
                        Key::Named(NamedKey::PageDown) => {
                            self.scroll_by(-page);
                            true
                        }
                        Key::Named(NamedKey::Home) => {
                            self.scroll_to(f32::MAX);
                            true
                        }
                        Key::Named(NamedKey::End) => {
                            self.scroll_to(0.0);
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                }

                let text = event.text.as_deref();
                if let Some(bytes) = self.key_bytes(&event.logical_key, text) {
                    self.send(&bytes);
                    if self.selection.take().is_some() {
                        self.needs_redraw = true;
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(quoted) = crate::input::quoted_path(&path) { self.paste(&quoted); }
                else { self.notify("This path contains unsupported characters"); }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.commands.is_some() || self.picker.is_some() { return; }
                if let Some(index) = self.tab_at(self.pointer) {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.request = Some(Request::ShowTab(index));
                    }
                    return;
                }
                let bypass = self.mouse_bypass || self.modifiers.shift_key();
                self.mouse_bypass = state == ElementState::Pressed && bypass;
                let at = self.mouse_at;
                let btn = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                if state == ElementState::Pressed {
                    self.mouse_held = Some(btn);
                } else {
                    self.mouse_held = None;
                }
                if self.mouse_grabbed() && !bypass {
                    self.mouse_report(btn, state == ElementState::Pressed, at.row, at.cell);
                    return;
                }
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        // Cmd-click (Ctrl-click on Linux) follows a link
                        // instead of selecting.
                        if PLATFORM.link(self.modifiers) {
                            if let Some((_, url)) = url_at(&self.term, at.line, at.cell) {
                                open_url(&url);
                                return;
                            }
                        }
                        let now = std::time::Instant::now();
                        self.clicks = match self.last_click {
                            Some((t, h)) if h == at && now.duration_since(t) < MULTI_CLICK => (self.clicks % 3) + 1,
                            _ => 1,
                        };
                        self.last_click = Some((now, at));
                        let block = self.modifiers.alt_key();
                        self.selection = match self.clicks {
                            // A word; a blank cell selects nothing.
                            2 => word_at(&self.term, at.line, at.cell).map(|w| Selection {
                                start: (w.line, w.start),
                                end: (w.line, w.end - 1),
                                block: false,
                            }),
                            // The whole logical line, soft wraps included.
                            3 => {
                                let (first, last) = logical_line(&self.term, at.line);
                                Some(Selection {
                                    start: (first, 0),
                                    end: (last, self.term.grid.cols.saturating_sub(1)),
                                    block: false,
                                })
                            }
                            _ => Some(Selection { start: (at.line, at.cell), end: (at.line, at.cell), block }),
                        };
                        self.selecting = self.clicks == 1;
                        self.needs_redraw = true;
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        self.selecting = false;
                        let t = self.selection.map(|s| selected_text(&self.term, s)).unwrap_or_default();
                        #[cfg(all(unix, not(target_os = "macos")))]
                        if !t.is_empty() {
                            if let Some(cb) = &mut self.clipboard {
                                let _ = crate::clipboard::set_primary(cb, t.clone());
                            }
                        }
                        if self.cfg.copy_on_select && !t.is_empty() {
                            if let Some(cb) = &mut self.clipboard {
                                let _ = cb.set_text(t);
                            }
                        }
                    }
                    (MouseButton::Middle, ElementState::Pressed) => {
                        self.paste_primary();
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = position;
                if self.commands.is_some() || self.picker.is_some() { return; }
                let hit = self.hit_test(position);
                if hit != self.mouse_at {
                    self.mouse_at = hit;
                    self.update_hover();
                    if self.mouse_grabbed() && !self.mouse_bypass && !self.modifiers.shift_key() {
                        // Viewport row, not absolute line.
                        self.motion_report(hit.row, hit.cell);
                        return;
                    }
                    if self.selecting {
                        if let Some(s) = &mut self.selection {
                            s.end = (hit.line, hit.cell);
                            self.needs_redraw = true;
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                if self.commands.is_some() || self.picker.is_some() { return; }
                // A gesture starting fresh must not inherit a stale remainder.
                if phase == TouchPhase::Started {
                    self.wheel_remainder = 0.0;
                }
                let grabbed = self.mouse_grabbed();
                // The multiplier is a convenience for discrete wheel notches.
                // It must not apply to a trackpad's continuous deltas, nor when
                // the movement is being reported to an application — inflating
                // a report tells the program the user scrolled further than
                // they did.
                let raw = crate::scroll::wheel_to_lines(
                    match delta {
                        MouseScrollDelta::LineDelta(_, y) => crate::scroll::WheelDelta::Lines(y),
                        MouseScrollDelta::PixelDelta(p) => {
                            crate::scroll::WheelDelta::Pixels(p.y as f32)
                        }
                    },
                    self.fonts.cell_h,
                    self.cfg.wheel,
                    grabbed,
                );
                if raw == 0.0 {
                    return;
                }
                // Carry sub-line movement across events, discarding the
                // remainder on a direction change so a stale opposite-sign
                // residue cannot swallow the first notch of the new direction.
                if raw.signum() != self.wheel_remainder.signum() {
                    self.wheel_remainder = 0.0;
                }
                self.wheel_remainder += raw;

                if self.modifiers.shift_key() {
                    // Shift is the escape hatch: never report, always scroll
                    // our own history. Consume the remainder wholesale, since
                    // the scroller carries fractions itself.
                    let lines = std::mem::take(&mut self.wheel_remainder);
                    self.scroll_wheel(lines);
                    return;
                }

                // Whole lines only from here; the fraction stays banked.
                let lines = self.wheel_remainder.trunc();
                if lines == 0.0 {
                    return;
                }
                self.wheel_remainder -= lines;
                let ticks = (lines.abs() as usize).clamp(1, 32);

                if grabbed {
                    let button = if lines > 0.0 {
                        crate::mouse::BTN_WHEEL_UP
                    } else {
                        crate::mouse::BTN_WHEEL_DOWN
                    };
                    let at = self.mouse_at;
                    // One report per line, which is how applications see wheel
                    // acceleration. The wheel is press-only.
                    for _ in 0..ticks {
                        self.mouse_report(button, true, at.row, at.cell);
                    }
                    return;
                }

                if self.term.grid.in_alt() {
                    // A full-screen program with no mouse grab: offer cursor
                    // keys, which is how `less` and `man` scroll.
                    if self.cfg.alternate_scroll && self.term.modes.contains(Modes::ALT_SCROLL) {
                        // Honour DECCKM: an application that put the keypad in
                        // application mode expects SS3, not CSI.
                        let app = self.term.modes.contains(Modes::APP_CURSOR);
                        let seq: &[u8] = match (lines > 0.0, app) {
                            (true, true) => b"\x1bOA",
                            (true, false) => b"\x1b[A",
                            (false, true) => b"\x1bOB",
                            (false, false) => b"\x1b[B",
                        };
                        let mut buf = Vec::with_capacity(ticks * 3);
                        for _ in 0..ticks {
                            buf.extend_from_slice(seq);
                        }
                        self.write_pty(&buf);
                    }
                    return;
                }

                self.scroll_wheel(lines);
            }
            WindowEvent::RedrawRequested => {
                let t0 = std::time::Instant::now();
                self.step_scroll();
                if self.search.is_some() && self.search_index.update(&self.term.grid, self.epoch) {
                    // Content changed under an open search: keep the hits
                    // honest without yanking the view around.
                    if let Some(s) = &mut self.search {
                        let near = s.current_line();
                        s.run(&self.search_index, near);
                    }
                }
                let t1 = std::time::Instant::now();
                let cursor_on = self.blink_on || !self.blinking();
                let (Some(win), Some(output)) = (&self.window, &mut self.output) else { return };
                let size = win.inner_size();
                if size.width == 0 || size.height == 0 {
                    return;
                }
                let bell = self
                    .bell_at
                    .map(|t| 1.0 - t.elapsed().as_secs_f32() / BELL_FLASH.as_secs_f32())
                    .filter(|b| *b > 0.0)
                    .unwrap_or(0.0);
                let bar = self.search.as_ref().map(|s| SearchBar {
                    query: &s.query,
                    total: s.matches.len(),
                    current: s.current,
                });
                let picker_themes = self.picker.as_ref().map(|p| p.matches()).unwrap_or_default();
                let picker = self.picker.as_ref().map(|p| PickerView {
                    query: &p.query,
                    themes: &picker_themes,
                    selected: p.selected,
                });
                let opts = DrawOpts {
                    selection: self.selection,
                    focused: self.focused,
                    cursor_on,
                    cursor_shape: self.cfg.cursor_shape,
                    bell,
                    hover_link: self.hover_link.as_ref().map(|l| l.0),
                    highlights: self.search.as_ref().map_or(&[], |s| s.spans.as_slice()),
                    current: self.search.as_ref().map_or(&[], |s| s.current_spans()),
                    search: bar,
                    picker,
                    commands: self.commands.as_ref(),
                    notice: self.notice.as_ref().map(|(text, _)| text.as_str()),
                    tabs: self.tabs.as_ref().map(|(titles, active)| TabBar { titles, active: *active }),
                };
                let t2;
                match output {
                    Output::Gpu(gpu) => {
                        gpu.resize(size.width, size.height);
                        if self.fonts.generation() != self.atlas_generation {
                            gpu.clear_glyphs();
                            self.atlas_generation = self.fonts.generation();
                        }
                        gpu.begin();
                        self.renderer.draw(gpu, &self.term, &mut self.fonts, &opts);
                        t2 = std::time::Instant::now();
                        if let Err(e) = gpu.flush(win) {
                            log::warn!("present failed: {e:#}");
                        }
                    }
                    Output::Software(surface) => {
                        let (Some(w), Some(h)) = (
                            std::num::NonZeroU32::new(size.width),
                            std::num::NonZeroU32::new(size.height),
                        ) else {
                            return;
                        };
                        if surface.resize(w, h).is_err() {
                            return;
                        }
                        let Ok(mut buffer) = surface.buffer_mut() else { return };
                        let mut canvas = Canvas {
                            buf: &mut buffer,
                            width: size.width as usize,
                            height: size.height as usize,
                        };
                        self.renderer.draw(&mut canvas, &self.term, &mut self.fonts, &opts);
                        t2 = std::time::Instant::now();
                        win.pre_present_notify();
                        let _ = buffer.present();
                    }
                }
                let t3 = std::time::Instant::now();
                self.frames.presented(t3);
                self.needs_redraw = false;
                if let Some(p) = &mut self.profile {
                    p.record([t1 - t0, t2 - t1, t3 - t2]);
                }

                let title = if self.term.title == "alef-term" { &self.session_title } else { &self.term.title };
                if win.title() != *title {
                    win.set_title(title);
                }
            }
            _ => {}
        }
    }

    /// Drain output, advance animations, and say when the loop should wake
    /// again for this pane (`None` when it can sleep until an event).
    fn tick(&mut self) -> Option<std::time::Instant> {
        self.drain_pty();
        if self.exited {
            return None;
        }
        // Only ask for another frame while something is actually moving; when
        // idle the loop goes back to blocking on events rather than spinning.
        let settled = self.scroll.at_bottom() || self.scroll.pending == 0.0;
        let animating = !settled
            || (self.scroll.target - self.scroll.pos).abs() > 0.01
            || self.renderer.scrollbar_alpha > 0.01 && self.scroll.at_bottom();
        let now = std::time::Instant::now();
        let mut deadline = self.notice.as_ref().map(|(_, until)| *until);
        if deadline.is_some_and(|until| now >= until) {
            self.notice = None;
            self.needs_redraw = true;
            deadline = None;
        }
        if self.blinking() {
            if now >= self.blink_at {
                self.blink_on = !self.blink_on;
                self.blink_at = now + BLINK_HALF;
                self.needs_redraw = true;
            }
            deadline = Some(deadline.map_or(self.blink_at, |d| d.min(self.blink_at)));
        } else if !self.blink_on {
            self.blink_on = true;
            self.needs_redraw = true;
        }
        let bell_active = self.bell_at.is_some_and(|t| t.elapsed() < BELL_FLASH);
        if !bell_active {
            self.bell_at = None;
        }
        let animating = animating || bell_active;
        // A tab not on show keeps reading its shell but draws nothing.
        if (self.needs_redraw || animating) && !self.occluded && self.output.is_some() {
            if let Some(w) = &self.window {
                let due = self.frames.deadline(now, self.refresh_millihertz);
                if due <= now {
                    w.request_redraw();
                } else {
                    deadline = Some(deadline.map_or(due, |d| d.min(due)));
                }
            }
        }
        if self.pty_pending {
            // The reader may already have queued all output and gone idle.
            // Explicitly continue after a budget yield, without depending on
            // another read to wake us or spinning before input is dispatched.
            let resume = now + std::time::Duration::from_millis(1);
            deadline = Some(deadline.map_or(resume, |d| d.min(resume)));
        }
        // A hold ends either when the application closes it or when the
        // watchdog fires. The latter needs the loop to wake on its own, since
        // a crashed program will send nothing more.
        if let Some(hold) = self.frame_hold.deadline() {
            deadline = Some(deadline.map_or(hold, |d| d.min(hold)));
        }
        deadline
    }
}

/// The application: one pane per window.
pub struct App {
    cfg: Config,
    proxy: EventLoopProxy<Wake>,
    panes: Vec<Pane>,
    next_group: u64,
    next_session: u64,
}

impl App {
    fn reload_settings(&mut self) -> anyhow::Result<()> {
        let cfg = crate::config::reload(&self.cfg)?;
        // Prepare every fallible part before changing any running window.
        let fonts = self.panes.iter().map(|pane| {
            if pane.cfg.font != cfg.font || pane.cfg.arabic_font != cfg.arabic_font
                || pane.cfg.arabic_scale != cfg.arabic_scale {
                Fonts::with_families(cfg.font_size * pane.scale, cfg.arabic_scale,
                    cfg.font.as_deref(), cfg.arabic_font.as_deref()).map(Some)
            } else { Ok(None) }
        }).collect::<anyhow::Result<Vec<_>>>()?;
        for (pane, fonts) in self.panes.iter_mut().zip(fonts) { pane.apply_config(&cfg, fonts); }
        self.cfg = cfg;
        Ok(())
    }

    fn set_arabic_font(&mut self, choice: crate::font::ArabicFont) -> anyhow::Result<()> {
        let family = choice.family();
        let fonts = self.panes.iter().map(|pane| {
            let fonts = Fonts::with_families(pane.cfg.font_size * pane.scale,
                pane.cfg.arabic_scale, pane.cfg.font.as_deref(), Some(family))?;
            if fonts.arabic_family != family {
                return Err(anyhow::anyhow!("{family} is not installed"));
            }
            Ok(fonts)
        }).collect::<anyhow::Result<Vec<_>>>()?;
        self.cfg.arabic_font = Some(family.to_string());
        for (pane, fonts) in self.panes.iter_mut().zip(fonts) {
            pane.apply_arabic_font(family, fonts);
        }
        if let Some(path) = &self.cfg.config_path {
            if let Err(error) = crate::config::save_arabic_font(path, family) {
                log::warn!("could not save Arabic font: {error:#}");
                for pane in &mut self.panes { pane.notify(format!("{family} active; could not save settings")); }
            }
        }
        Ok(())
    }

    pub fn new(cfg: Config, proxy: EventLoopProxy<Wake>) -> anyhow::Result<App> {
        // Fail early on what every pane needs, before the event loop starts.
        Fonts::with_families(cfg.font_size, cfg.arabic_scale, cfg.font.as_deref(), cfg.arabic_font.as_deref())?;
        Ok(App { cfg, proxy, panes: Vec::new(), next_group: 0, next_session: 0 })
    }

    /// Panes sharing pane `i`'s window, in tab order.
    fn tabs_of(&self, i: usize) -> Vec<usize> {
        let id = self.panes[i].window_id();
        (0..self.panes.len()).filter(|&j| self.panes[j].window_id() == id).collect()
    }

    /// Tell each pane about the tabs in its window, for the tab bar. A window
    /// with one tab has no bar.
    fn refresh_tabs(&mut self) {
        for i in 0..self.panes.len() {
            let tabs = self.tabs_of(i);
            let bar = (tabs.len() > 1).then(|| {
                let titles = tabs.iter().map(|&j| self.panes[j].tab_title()).collect();
                let active = tabs.iter().position(|&j| self.panes[j].output.is_some()).unwrap_or(0);
                (titles, active)
            });
            self.panes[i].set_tabs(bar);
        }
    }

    /// Show tab `to` in place of `from`, in the same window: the surface moves
    /// across, and so do focus and the modifier keys held.
    fn switch_tab(&mut self, from: usize, to: usize) {
        if from == to || self.panes[from].output.is_none() {
            return;
        }
        let output = self.panes[from].output.take();
        let (focused, modifiers, occluded) = (self.panes[from].focused, self.panes[from].modifiers, self.panes[from].occluded);
        if focused {
            self.panes[from].set_focus(false);
        }
        let pane = &mut self.panes[to];
        pane.output = output;
        // Glyph ids are numbered per pane, so the atlas holds another pane's.
        pane.atlas_generation = u64::MAX;
        pane.modifiers = modifiers;
        pane.occluded = occluded;
        if focused {
            pane.set_focus(true);
        }
        // The window may have been resized while this tab was hidden.
        pane.reflow();
        if let Some(w) = &pane.window {
            w.request_redraw();
        }
    }

    /// Open a tab in pane `i`'s window, just after it, and show it.
    fn open_tab(&mut self, i: usize, start_dir: Option<String>) {
        let Some(win) = self.panes[i].window.clone() else { return };
        if self.panes[i].output.is_none() {
            return;
        }
        let mut cfg = self.cfg.clone();
        cfg.startup_cmd = None;
        cfg.startup_warning = None;
        let start_dir = start_dir.or_else(|| cfg.working_directory.clone());
        let mut pane = match Pane::new(cfg, self.proxy.clone(), start_dir) {
            Ok(pane) => pane,
            Err(e) => {
                self.panes[i].notify(format!("Cannot open a tab: {e}"));
                return;
            }
        };
        self.next_session += 1;
        pane.session_title = format!("Shell {}", self.next_session);
        pane.tab_group = self.panes[i].tab_group.clone();
        // Lay the new shell out below the bar that is about to appear.
        pane.tabs = Some((Vec::new(), 0));
        pane.set_display_scale(win.scale_factor() as f32);
        pane.atlas_generation = u64::MAX;
        let output = self.panes[i].output.take();
        pane.start(win, output.expect("checked above"));
        if pane.exited {
            self.panes[i].output = pane.output.take();
            self.panes[i].needs_redraw = true;
            return;
        }
        let (focused, modifiers) = (self.panes[i].focused, self.panes[i].modifiers);
        if focused {
            self.panes[i].set_focus(false);
        }
        pane.focused = focused;
        pane.modifiers = modifiers;
        self.panes.insert(i + 1, pane);
        self.refresh_tabs();
    }

    /// Close pane `i`. A tab on show first hands the window to a neighbour,
    /// so the window stays open while it has tabs.
    fn close_pane(&mut self, i: usize) {
        let tabs = self.tabs_of(i);
        if tabs.len() > 1 && self.panes[i].output.is_some() {
            let at = tabs.iter().position(|&j| j == i).unwrap_or(0);
            let next = if at + 1 < tabs.len() { tabs[at + 1] } else { tabs[at - 1] };
            self.switch_tab(i, next);
        }
        self.panes.remove(i);
        self.refresh_tabs();
    }

    fn open_pane(&mut self, el: &ActiveEventLoop, start_dir: Option<String>, tab_group: Option<String>) {
        let group = tab_group.unwrap_or_else(|| {
            self.next_group += 1;
            format!("alef-term-{}-{}", std::process::id(), self.next_group)
        });
        let mut cfg = self.cfg.clone();
        // A startup command belongs to the first window only.
        cfg.startup_cmd = self.cfg.startup_cmd.take();
        cfg.startup_warning = self.cfg.startup_warning.take();
        let start_dir = start_dir.or_else(|| cfg.working_directory.clone());
        match Pane::new(cfg, self.proxy.clone(), start_dir) {
            Ok(mut pane) => {
                pane.tab_group = group;
                self.next_session += 1;
                pane.session_title = format!("Shell {}", self.next_session);
                pane.open(el);
                if !pane.exited {
                    self.panes.push(pane);
                }
            }
            Err(e) => eprintln!("alef-term: cannot open a window: {e:#}"),
        }
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.panes.is_empty() {
            self.open_pane(el, None, None);
            if self.panes.is_empty() {
                el.exit();
            }
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _: Wake) {
        // Output is drained once in about_to_wait, after input events have
        // been delivered. A wake must not itself start another parse/redraw.
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            // The window's close button closes every tab in it.
            self.panes.retain(|p| p.window_id() != Some(id));
            self.refresh_tabs();
            if self.panes.is_empty() {
                el.exit();
            }
            return;
        }
        // Events go to the tab on show; the others in its window are hidden.
        let Some(i) = self.panes.iter().position(|p| p.window_id() == Some(id) && p.output.is_some()) else { return };
        self.panes[i].handle(event);
        match self.panes[i].request.take() {
            Some(Request::NewWindow) => {
                // A new window starts where this one's shell is.
                let dir = self.panes[i].term.cwd.clone();
                self.open_pane(el, dir, None);
            }
            Some(Request::NewTab) if PLATFORM == Platform::Linux => {
                let dir = self.panes[i].term.cwd.clone();
                self.open_tab(i, dir);
            }
            Some(Request::ShowTab(index)) => {
                if let Some(&to) = self.tabs_of(i).get(index) {
                    self.switch_tab(i, to);
                    self.refresh_tabs();
                }
            }
            Some(Request::NewTab) => {
                let dir = self.panes[i].term.cwd.clone();
                let group = self.panes[i].tab_group.clone();
                self.open_pane(el, dir, Some(group));
            }
            Some(Request::Close) => {
                // Dropping the pane ends its shell, and closes its window
                // with its last tab.
                self.close_pane(i);
                if self.panes.is_empty() {
                    el.exit();
                }
            }
            Some(Request::Quit) => el.exit(),
            Some(Request::Reload) => {
                if let Err(error) = self.reload_settings() {
                    log::warn!("settings reload failed: {error:#}");
                    self.panes[i].notify(format!("Settings unchanged: {error}"));
                }
            }
            Some(Request::SetArabicFont(font)) => {
                if let Err(error) = self.set_arabic_font(font) {
                    self.panes[i].notify(format!("Font unchanged: {error}"));
                }
            }
            Some(Request::SetTheme(name)) => {
                let Some(t) = find_theme(name) else { return };
                // New windows start with it too.
                self.cfg.set_theme(t);
                for pane in &mut self.panes {
                    pane.apply_theme(t);
                }
                if let Some(path) = &self.cfg.config_path {
                    if let Err(e) = crate::config::save_theme(path, t.name) {
                        eprintln!("alef-term: could not save the theme: {e:#}");
                    }
                }
            }
            None => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let mut deadline: Option<std::time::Instant> = None;
        for pane in &mut self.panes {
            let due = pane.tick();
            if !pane.exited {
                deadline = match (deadline, due) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
        }
        while let Some(i) = self.panes.iter().position(|p| p.exited) {
            self.close_pane(i);
        }
        // Titles change as programs set them, in hidden tabs too.
        self.refresh_tabs();
        if self.panes.is_empty() {
            el.exit();
            return;
        }
        el.set_control_flow(match deadline {
            Some(due) => ControlFlow::WaitUntil(due),
            None => ControlFlow::Wait,
        });
    }
}
