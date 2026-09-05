//! Window, input handling, and the frame loop.

use crate::color::Palette;
use crate::font::Fonts;
use crate::pty::{Pty, PtyEvent};
use crate::render::{Canvas, Renderer};
use crate::selection::{selected_text, Selection};
use crate::term::{BidiMode, Modes, Terminal, Vt};
use std::num::NonZeroU32;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

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
        }
    }
}

pub struct App {
    cfg: Config,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    term: Terminal,
    vt: Vt,
    pty: Option<Pty>,
    fonts: Fonts,
    renderer: Renderer,
    palette: Palette,
    proxy: EventLoopProxy<Wake>,
    modifiers: ModifiersState,
    scale: f32,
    focused: bool,
    selection: Option<Selection>,
    selecting: bool,
    mouse_cell: (usize, usize),
    clipboard: Option<arboard::Clipboard>,
    needs_redraw: bool,
    exited: bool,
    scroll: crate::scroll::Scroller,
    last_frame: Option<std::time::Instant>,
    scrollbar_fade: f32,
}

impl App {
    pub fn new(cfg: Config, proxy: EventLoopProxy<Wake>) -> anyhow::Result<App> {
        let fonts = Fonts::new(cfg.font_size, cfg.arabic_scale)?;
        let term = Terminal::new(cfg.rows, cfg.cols, cfg.scrollback);
        Ok(App {
            cfg,
            window: None,
            surface: None,
            term,
            vt: Vt::new(),
            pty: None,
            fonts,
            renderer: Renderer::new(),
            palette: Palette::dark(),
            proxy,
            modifiers: ModifiersState::empty(),
            scale: 1.0,
            focused: true,
            selection: None,
            selecting: false,
            mouse_cell: (0, 0),
            clipboard: arboard::Clipboard::new().ok(),
            needs_redraw: true,
            exited: false,
            scroll: Default::default(),
            last_frame: None,
            scrollbar_fade: 0.0,
        })
    }

    fn cell_size(&self) -> (f32, f32) {
        (self.fonts.cell_w, self.fonts.cell_h)
    }

    /// Recompute the grid from the window size and tell the child about it.
    fn reflow(&mut self) {
        let Some(win) = &self.window else { return };
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
        // Coalesce everything queued into one parse pass; under a flood this
        // keeps the event loop responsive instead of redrawing per chunk.
        let mut batch: Vec<u8> = Vec::new();
        let mut exited = false;
        while let Ok(ev) = pty.rx.try_recv() {
            match ev {
                PtyEvent::Output(mut b) => {
                    batch.append(&mut b);
                    if batch.len() > 4 << 20 {
                        break;
                    }
                }
                PtyEvent::Exited => exited = true,
            }
        }
        if !batch.is_empty() {
            self.vt.feed(&mut self.term, &batch);
            // New output means the user is looking at live content again.
            self.scroll.snap_to_bottom();
            self.term.grid.view_offset = 0;
            self.renderer.scroll_frac = 0.0;
        }
        for (idx, rgb) in self.term.take_palette_overrides() {
            self.palette.colors[idx as usize] = rgb;
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
        if self.term.damaged {
            self.term.damaged = false;
            self.needs_redraw = true;
        }
        if exited {
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

        let max = self.term.grid.scrollback.len() as f32;
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

    /// Scroll by a number of lines. Positive moves back into history.
    fn scroll_by(&mut self, lines: f32) {
        self.scroll.nudge(lines);
        self.needs_redraw = true;
    }

    fn scroll_to(&mut self, lines: f32) {
        let max = self.term.grid.scrollback.len() as f32;
        self.scroll.jump_to(lines, max);
        self.needs_redraw = true;
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Some(p) = &mut self.pty {
            p.write(bytes);
        }
        if !self.scroll.at_bottom() {
            // Typing returns to the live screen, as every terminal does.
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

    /// Map a pixel position to a logical (row, cell), going through the row's
    /// visual->logical column map so clicks land on the character the user sees.
    fn hit_test(&self, pos: PhysicalPosition<f64>) -> (usize, usize) {
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
        (row, cell)
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

    fn mouse_report(&mut self, button: u8, pressed: bool, row: usize, col: usize) {
        if !self.term.modes.intersects(Modes::MOUSE_CLICK | Modes::MOUSE_DRAG | Modes::MOUSE_MOTION)
        {
            return;
        }
        let (r, c) = (row + 1, col + 1);
        let seq = if self.term.modes.contains(Modes::MOUSE_SGR) {
            format!("\x1b[<{};{};{}{}", button, c, r, if pressed { 'M' } else { 'm' })
        } else {
            let b = if pressed { button } else { 3 };
            format!(
                "\x1b[M{}{}{}",
                (32 + b) as char,
                (32 + c.min(223)) as u8 as char,
                (32 + r.min(223)) as u8 as char
            )
        };
        self.send(seq.as_bytes());
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

        let ctx = softbuffer::Context::new(win.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&ctx, win.clone()).expect("softbuffer surface");
        self.surface = Some(surface);
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

    fn user_event(&mut self, el: &ActiveEventLoop, _: Wake) {
        self.drain_pty();
        if self.exited {
            el.exit();
            return;
        }
        if self.needs_redraw {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
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
                    self.send(if f { b"\x1b[I" } else { b"\x1b[O" });
                }
                self.needs_redraw = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor as f32;
                self.fonts.set_size(self.cfg.font_size * self.scale);
                self.reflow();
            }
            WindowEvent::Resized(_) => self.reflow(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Ime(winit::event::Ime::Commit(s)) => {
                self.send(s.as_bytes());
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
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
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
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
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
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
                let (row, cell) = self.mouse_cell;
                let btn = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                if self.term.modes.intersects(Modes::MOUSE_CLICK | Modes::MOUSE_DRAG) {
                    self.mouse_report(btn, state == ElementState::Pressed, row, cell);
                    return;
                }
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.selecting = true;
                        self.selection = Some(Selection {
                            start: (row, cell),
                            end: (row, cell),
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
                if hit != self.mouse_cell {
                    self.mouse_cell = hit;
                    if self.selecting {
                        if let Some(s) = &mut self.selection {
                            s.end = hit;
                            self.needs_redraw = true;
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    // A trackpad reports pixels; convert with the real cell
                    // height so a gesture moves the same distance at any size.
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / self.fonts.cell_h.max(1.0),
                };
                if lines == 0.0 {
                    return;
                }
                if self.term.grid.in_alt() {
                    // Full-screen programs get arrow keys; they have no
                    // scrollback of our own to move through.
                    let n = (lines.abs().ceil() as usize).min(5);
                    let seq: &[u8] = if lines > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
                    let mut buf = Vec::with_capacity(n * 3);
                    for _ in 0..n {
                        buf.extend_from_slice(seq);
                    }
                    self.send(&buf);
                    return;
                }
                // Accumulate only; the frame applies it. Many events between
                // two frames cost one redraw, not one each.
                self.scroll.nudge(lines * 3.0);
                self.needs_redraw = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.drain_pty();
                self.step_scroll();
                let (Some(win), Some(surface)) = (&self.window, &mut self.surface) else { return };
                let size = win.inner_size();
                let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
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
                self.renderer.draw(
                    &mut canvas,
                    &self.term,
                    &mut self.fonts,
                    &self.palette,
                    self.selection,
                    self.focused,
                );
                let _ = buffer.present();
                self.needs_redraw = false;

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
        if self.needs_redraw || animating {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}
