//! Window, input handling, and the frame loop.

use crate::font::Fonts;
use crate::frame::FramePacer;
use crate::pty::Pty;
use crate::gpu::Gpu;
use crate::render::{DrawOpts, Renderer};
use crate::selection::{selected_text, Selection};
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

pub struct Config {
    pub font_size: f32,
    /// `None` derives the Arabic scale from font metrics.
    pub arabic_scale: Option<f32>,
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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font_size: 15.0,
            arabic_scale: None,
            scrollback: 10_000,
            cols: 96,
            rows: 28,
            startup_cmd: None,
            wheel: Default::default(),
            alternate_scroll: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            visual_bell: true,
        }
    }
}

pub struct App {
    cfg: Config,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
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
    /// Frame timing, enabled with `ARABIC_TERM_PROFILE=1`.
    profile: Option<Profile>,
    /// Cursor blink: the current phase and when it next flips. Typing resets
    /// the phase to visible, so the cursor never vanishes under the fingers.
    blink_on: bool,
    blink_at: std::time::Instant,
    /// When the last bell rang, for the flash.
    bell_at: Option<std::time::Instant>,
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

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<Wake>) -> anyhow::Result<App> {
        let fonts = Fonts::new(cfg.font_size, cfg.arabic_scale)?;
        let term = Terminal::new(cfg.rows, cfg.cols, cfg.scrollback);
        Ok(App {
            cfg,
            window: None,
            gpu: None,
            atlas_generation: 0,
            term,
            vt: Vt::new(),
            pty: None,
            fonts,
            renderer: Renderer::new(),
            proxy,
            modifiers: ModifiersState::empty(),
            scale: 1.0,
            focused: true,
            selection: None,
            selecting: false,
            mouse_at: Hit::default(),
            mouse_held: None,
            clipboard: arboard::Clipboard::new().ok(),
            needs_redraw: true,
            exited: false,
            frame_hold: crate::frame_hold::FrameHold::new(),
            scroll: Default::default(),
            wheel_remainder: 0.0,
            last_frame: None,
            scrollbar_fade: 0.0,
            profile: std::env::var_os("ARABIC_TERM_PROFILE").map(|_| Profile::new()),
            blink_on: true,
            blink_at: std::time::Instant::now() + BLINK_HALF,
            bell_at: None,
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
            && self.focused
            && self.term.modes.contains(Modes::CURSOR_VISIBLE)
            && self.term.grid.view_offset == 0
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
        let rows = (((size.height as f32 - 2.0 * py) / ch).floor() as usize).max(1);
        if cols != self.term.grid.cols || rows != self.term.grid.rows {
            self.term.resize(rows, cols);
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
        if std::mem::take(&mut self.term.bells) > 0 && self.cfg.visual_bell {
            self.bell_at = Some(std::time::Instant::now());
            self.needs_redraw = true;
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
        let (px, py) = (self.renderer.pad_x, self.renderer.pad_y);
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

    fn paste(&mut self, text: &str) {
        let bracketed = self.term.modes.contains(Modes::BRACKETED_PASTE);
        // Strip control characters that could inject commands from a paste.
        let cleaned: String =
            text.chars().filter(|c| *c == '\t' || *c == '\n' || !c.is_control()).collect();
        let mut buf = Vec::new();
        if bracketed {
            buf.extend_from_slice(b"\x1b[200~");
        }
        buf.extend_from_slice(cleaned.replace('\n', "\r").as_bytes());
        if bracketed {
            buf.extend_from_slice(b"\x1b[201~");
        }
        self.send(&buf);
    }

    fn set_font_size(&mut self, size: f32) {
        self.cfg.font_size = size.clamp(6.0, 72.0);
        self.fonts.set_size(self.cfg.font_size * self.scale);
        self.reflow();
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

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (cw, ch) = self.cell_size();
        let attrs = Window::default_attributes()
            .with_title("arabic-term")
            .with_inner_size(LogicalSize::new(
                self.cfg.cols as f32 * cw,
                self.cfg.rows as f32 * ch,
            ));
        let win = Arc::new(el.create_window(attrs).expect("create window"));
        win.set_ime_allowed(true); // needed for Arabic input methods

        self.scale = win.scale_factor() as f32;
        self.fonts.set_size(self.cfg.font_size * self.scale);

        self.gpu = Some(Gpu::new(win.clone()).expect("initialise GPU"));
        self.atlas_generation = self.fonts.generation();
        self.window = Some(win);

        let (cw, ch) = self.cell_size();
        let proxy = self.proxy.clone();
        match Pty::spawn(
            None,
            self.term.grid.rows as u16,
            self.term.grid.cols as u16,
            cw as u16,
            ch as u16,
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
                el.exit();
            }
        }
        self.reflow();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _: Wake) {
        // Output is drained once in about_to_wait, after input events have
        // been delivered. A wake must not itself start another parse/redraw.
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(p) = &self.pty {
                    p.kill();
                }
                el.exit();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                if self.term.modes.contains(Modes::FOCUS_EVENTS) {
                    self.write_pty(if f { b"\x1b[I" } else { b"\x1b[O" });
                }
                self.needs_redraw = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.fonts.set_size(self.cfg.font_size * self.scale);
                self.reflow();
            }
            WindowEvent::Resized(_) => self.reflow(),
            WindowEvent::Moved(_) => {
                self.refresh_millihertz = self.window.as_ref()
                    .and_then(|w| w.current_monitor())
                    .and_then(|m| m.refresh_rate_millihertz());
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Ime(winit::event::Ime::Commit(s)) => {
                self.send(s.as_bytes());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                self.blink_reset();
                let sup = self.modifiers.super_key();
                if sup {
                    // macOS command bindings.
                    if let Key::Character(c) = &event.logical_key {
                        match c.as_str() {
                            "c" => {
                                let t = self.selection.map(|s| selected_text(&self.term, s)).unwrap_or_default();
                                if !t.is_empty() {
                                    if let Some(cb) = &mut self.clipboard {
                                        let _ = cb.set_text(t);
                                    }
                                }
                                return;
                            }
                            "v" => {
                                let text =
                                    self.clipboard.as_mut().and_then(|cb| cb.get_text().ok());
                                if let Some(t) = text {
                                    self.paste(&t);
                                }
                                return;
                            }
                            "+" | "=" => {
                                self.set_font_size(self.cfg.font_size + 1.0);
                                return;
                            }
                            "-" => {
                                self.set_font_size(self.cfg.font_size - 1.0);
                                return;
                            }
                            "0" => {
                                self.set_font_size(15.0);
                                return;
                            }
                            // Cycle the base-direction policy: useful when a
                            // program's output guesses wrong.
                            "r" => {
                                self.term.bidi_mode = match self.term.bidi_mode {
                                    BidiMode::ForceLtr => BidiMode::ForceRtl,
                                    BidiMode::ForceRtl => BidiMode::Auto,
                                    BidiMode::Auto => BidiMode::ForceLtr,
                                };
                                self.needs_redraw = true;
                                return;
                            }
                            _ => {}
                        }
                    }
                    return;
                }
                // Shift + navigation keys drive our scrollback rather than
                // being sent on, matching xterm and every terminal since.
                if self.modifiers.shift_key() && !self.term.grid.in_alt() {
                    let page = self.term.grid.rows.saturating_sub(1) as f32;
                    let handled = match &event.logical_key {
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
            WindowEvent::MouseInput { state, button, .. } => {
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
                if self.mouse_grabbed() {
                    self.mouse_report(btn, state == ElementState::Pressed, at.row, at.cell);
                    return;
                }
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.selecting = true;
                        self.selection = Some(Selection {
                            start: (at.line, at.cell),
                            end: (at.line, at.cell),
                            block: self.modifiers.alt_key(),
                        });
                        self.needs_redraw = true;
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        self.selecting = false;
                        let t = self.selection.map(|s| selected_text(&self.term, s)).unwrap_or_default();
                        if !t.is_empty() {
                            if let Some(cb) = &mut self.clipboard {
                                let _ = cb.set_text(t);
                            }
                        }
                    }
                    (MouseButton::Middle, ElementState::Pressed) => {
                        let text = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok());
                        if let Some(t) = text {
                            self.paste(&t);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let hit = self.hit_test(position);
                if hit != self.mouse_at {
                    self.mouse_at = hit;
                    if self.mouse_grabbed() {
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
                let t1 = std::time::Instant::now();
                let cursor_on = self.blink_on || !self.blinking();
                let (Some(win), Some(gpu)) = (&self.window, &mut self.gpu) else { return };
                let size = win.inner_size();
                if size.width == 0 || size.height == 0 {
                    return;
                }
                gpu.resize(size.width, size.height);
                if self.fonts.generation() != self.atlas_generation {
                    gpu.clear_glyphs();
                    self.atlas_generation = self.fonts.generation();
                }
                gpu.begin();
                let bell = self
                    .bell_at
                    .map(|t| 1.0 - t.elapsed().as_secs_f32() / BELL_FLASH.as_secs_f32())
                    .filter(|b| *b > 0.0)
                    .unwrap_or(0.0);
                let opts = DrawOpts {
                    selection: self.selection,
                    focused: self.focused,
                    cursor_on,
                    cursor_shape: self.cfg.cursor_shape,
                    bell,
                };
                self.renderer.draw(gpu, &self.term, &mut self.fonts, &opts);
                let t2 = std::time::Instant::now();
                if let Err(e) = gpu.flush(win) {
                    log::warn!("present failed: {e:#}");
                }
                let t3 = std::time::Instant::now();
                self.frames.presented(t3);
                self.needs_redraw = false;
                if let Some(p) = &mut self.profile {
                    p.record([t1 - t0, t2 - t1, t3 - t2]);
                }

                if win.title() != self.term.title {
                    win.set_title(&self.term.title);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain_pty();
        if self.exited {
            el.exit();
            return;
        }
        // Only ask for another frame while something is actually moving; when
        // idle the loop goes back to blocking on events rather than spinning.
        let settled = self.scroll.at_bottom() || self.scroll.pending == 0.0;
        let animating = !settled
            || (self.scroll.target - self.scroll.pos).abs() > 0.01
            || self.renderer.scrollbar_alpha > 0.01 && self.scroll.at_bottom();
        let now = std::time::Instant::now();
        let mut deadline = None;
        if self.blinking() {
            if now >= self.blink_at {
                self.blink_on = !self.blink_on;
                self.blink_at = now + BLINK_HALF;
                self.needs_redraw = true;
            }
            deadline = Some(self.blink_at);
        } else if !self.blink_on {
            self.blink_on = true;
            self.needs_redraw = true;
        }
        let bell_active = self.bell_at.is_some_and(|t| t.elapsed() < BELL_FLASH);
        if !bell_active {
            self.bell_at = None;
        }
        let animating = animating || bell_active;
        if self.needs_redraw || animating {
            if let Some(w) = &self.window {
                let due = self.frames.deadline(now, self.refresh_millihertz);
                if due <= now {
                    w.request_redraw();
                } else {
                    deadline = Some(due);
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
        el.set_control_flow(match deadline {
            Some(due) => ControlFlow::WaitUntil(due),
            None => ControlFlow::Wait,
        });
    }
}
