//! The configuration file.
//!
//! Read from `$ALEF_TERM_CONFIG`, or `~/.config/alef-term/config.toml`.
//! Every key is optional and the file may be absent; command-line flags
//! override whatever the file says. Unknown keys are an error, so a typo is
//! reported rather than silently ignored.

use crate::app::Config;
use crate::color::{find_theme, Overrides, Rgb, THEMES};
use crate::term::{BidiMode, CursorShape};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct File {
    /// Monospace family, tried before the built-in preference list.
    pub font: Option<String>,
    /// Arabic family, likewise.
    pub arabic_font: Option<String>,
    pub font_size: Option<f32>,
    /// Extra scale on the Arabic face; omitted derives it from the metrics.
    pub arabic_scale: Option<f32>,
    pub line_height: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    /// "auto", "ltr" or "rtl".
    pub bidi_mode: Option<String>,
    pub copy_on_select: Option<bool>,
    /// Programming ligatures in Latin text; on unless set to false.
    pub ligatures: Option<bool>,
    /// macOS window tabs; false draws the tab bar inside one window instead,
    /// which a tiling window manager needs.
    pub native_tabs: Option<bool>,
    /// Text contrast ratio, 1 disables adjustment; background colours stay exact.
    pub minimum_contrast: Option<f32>,
    /// Directory for the first window; new windows inherit OSC 7 when available.
    pub working_directory: Option<String>,
    pub scrollback: Option<usize>,
    pub cols: Option<usize>,
    pub rows: Option<usize>,
    /// Lines per wheel notch.
    pub scroll_lines: Option<f32>,
    /// Trackpad scroll multiplier.
    pub touch_scroll: Option<f32>,
    /// Turn the wheel into arrow keys for full-screen programs.
    pub alternate_scroll: Option<bool>,
    /// "block", "underline" or "bar".
    pub cursor_shape: Option<String>,
    pub cursor_blink: Option<bool>,
    pub visual_bell: Option<bool>,
    /// A built-in theme, by name (`alef-term --list-themes`); the base the
    /// colours below adjust.
    pub theme: Option<String>,
    /// "gpu" (the default) or "software".
    pub renderer: Option<String>,
    pub colors: Colors,
}

/// Colour overrides, as `#rrggbb`.
#[derive(Clone, Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor: Option<String>,
    pub selection_background: Option<String>,
    pub selection_foreground: Option<String>,
    /// The 16 ANSI colours in order: black, red, green, yellow, blue,
    /// magenta, cyan, white, then their bright forms. Fewer than 16 entries
    /// override only the first ones.
    pub ansi: Vec<String>,
}

/// Where the file lives, whether or not it exists.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ALEF_TERM_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".config/alef-term/config.toml"))
}

/// Parse the file at `path`. A missing file is an empty configuration.
pub fn load(path: &Path) -> Result<File> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    parse(&text).with_context(|| format!("in {}", path.display()))
}

pub fn parse(text: &str) -> Result<File> {
    toml::from_str(text).map_err(|e| anyhow!("{e}"))
}

/// `#rrggbb` (the `#` optional) to a colour.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn color(name: &str, s: &Option<String>) -> Result<Option<Rgb>> {
    s.as_ref()
        .map(|s| parse_hex(s).ok_or_else(|| anyhow!("colors.{name}: expected #rrggbb, got {s:?}")))
        .transpose()
}

/// Record `theme` as the configured theme and change nothing else in the
/// file. A file that does not parse is left alone rather than clobbered; one
/// that does not exist is created.
pub fn save_theme(path: &Path, theme: &str) -> Result<()> {
    save_string(path, "theme", theme)
}

pub fn save_arabic_font(path: &Path, family: &str) -> Result<()> {
    save_string(path, "arabic_font", family)
}

fn save_string(path: &Path, key: &str, value: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    parse(&text).with_context(|| format!("{} has an error; not saving {key}", path.display()))?;
    let updated = set_string_line(&text, key, value);
    parse(&updated).context("the updated configuration does not parse")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, updated).with_context(|| format!("writing {}", path.display()))
}

/// `text` with its top-level `theme` key set to `theme`: the existing line is
/// rewritten in place, keeping any comment after it, or a new one goes above
/// the first table.
pub fn set_theme_line(text: &str, theme: &str) -> String {
    set_string_line(text, "theme", theme)
}

fn set_string_line(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", toml::Value::String(value.to_string()));
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for l in text.lines() {
        let t = l.trim_start();
        let is_key = t.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
        if !done && is_key {
            // Preset names contain no `#`; retain the existing trailing comment.
            out.push(match l.find('#') {
                Some(at) => format!("{line:<width$}{}", &l[at..], width = at.max(line.len() + 1)),
                None => line.clone(),
            });
            done = true;
            continue;
        }
        if !done && t.starts_with('[') {
            // Top-level keys must come before any table.
            out.push(line.clone());
            out.push(String::new());
            done = true;
        }
        out.push(l.to_string());
    }
    if !done {
        out.push(line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

impl File {
    /// Fold the file into the runtime configuration.
    pub fn apply(&self, cfg: &mut Config) -> Result<()> {
        let mut next = cfg.clone();
        self.apply_validated(&mut next)?;
        *cfg = next;
        Ok(())
    }

    fn apply_validated(&self, cfg: &mut Config) -> Result<()> {
        if self.font.is_some() {
            cfg.font = self.font.clone();
        }
        if self.arabic_font.is_some() {
            cfg.arabic_font = self.arabic_font.clone();
        }
        if let Some(v) = self.font_size {
            if !(6.0..=72.0).contains(&v) {
                return Err(anyhow!("font_size: {v} is out of range"));
            }
            cfg.font_size = v;
        }
        if let Some(v) = self.arabic_scale {
            if !(0.5..=2.5).contains(&v) {
                return Err(anyhow!("arabic_scale: expected 0.5..=2.5, got {v}"));
            }
            cfg.arabic_scale = Some(v);
        }
        if let Some(v) = self.line_height {
            if !(1.0..=2.0).contains(&v) {
                return Err(anyhow!("line_height: expected 1.0..=2.0, got {v}"));
            }
            cfg.line_height = v;
        }
        for (name, value, dest) in [
            ("padding_x", self.padding_x, &mut cfg.padding_x),
            ("padding_y", self.padding_y, &mut cfg.padding_y),
        ] {
            if let Some(v) = value {
                if !(0.0..=100.0).contains(&v) {
                    return Err(anyhow!("{name}: expected 0..=100 logical pixels, got {v}"));
                }
                *dest = v;
            }
        }
        if let Some(mode) = &self.bidi_mode {
            cfg.bidi_mode = match mode.as_str() {
                "auto" => BidiMode::Auto,
                "ltr" => BidiMode::ForceLtr,
                "rtl" => BidiMode::ForceRtl,
                _ => return Err(anyhow!("bidi_mode: expected auto, ltr or rtl, got {mode:?}")),
            };
        }
        if let Some(v) = self.scrollback {
            if v > 1_000_000 { return Err(anyhow!("scrollback: maximum is 1000000 lines")); }
            cfg.scrollback = v;
        }
        if let Some(v) = self.cols {
            if !(2..=1000).contains(&v) { return Err(anyhow!("cols: expected 2..=1000")); }
            cfg.cols = v;
        }
        if let Some(v) = self.rows {
            if !(1..=1000).contains(&v) { return Err(anyhow!("rows: expected 1..=1000")); }
            cfg.rows = v;
        }
        if let Some(v) = self.scroll_lines {
            if !(0.0..=100.0).contains(&v) { return Err(anyhow!("scroll_lines: expected 0..=100")); }
            cfg.wheel.wheel = v;
        }
        if let Some(v) = self.touch_scroll {
            if !(0.0..=100.0).contains(&v) { return Err(anyhow!("touch_scroll: expected 0..=100")); }
            cfg.wheel.touch = v;
        }
        if let Some(v) = self.copy_on_select { cfg.copy_on_select = v; }
        if let Some(v) = self.ligatures { cfg.ligatures = v; }
        if let Some(v) = self.native_tabs { cfg.native_tabs = v; }
        if let Some(v) = self.minimum_contrast {
            if !(1.0..=21.0).contains(&v) { return Err(anyhow!("minimum_contrast: expected 1..=21")); }
            cfg.minimum_contrast = v;
        }
        if let Some(dir) = &self.working_directory {
            let path = if dir == "~" || dir.starts_with("~/") {
                let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("working_directory: HOME is not set"))?;
                PathBuf::from(home).join(dir.strip_prefix("~/").unwrap_or(""))
            } else { PathBuf::from(dir) };
            if !path.is_dir() { return Err(anyhow!("working_directory: {} is not a directory", path.display())); }
            cfg.working_directory = Some(path.to_string_lossy().into_owned());
        }
        if let Some(v) = self.alternate_scroll {
            cfg.alternate_scroll = v;
        }
        if let Some(s) = &self.cursor_shape {
            cfg.cursor_shape = match s.as_str() {
                "block" => CursorShape::Block,
                "underline" => CursorShape::Underline,
                "bar" | "beam" => CursorShape::Bar,
                other => return Err(anyhow!("cursor_shape: expected block, underline or bar, got {other:?}")),
            };
        }
        if let Some(v) = self.cursor_blink {
            cfg.cursor_blink = v;
        }
        if let Some(v) = self.visual_bell {
            cfg.visual_bell = v;
        }
        if let Some(r) = &self.renderer {
            cfg.software = match r.as_str() {
                "gpu" => false,
                "software" | "cpu" => true,
                other => return Err(anyhow!("renderer: expected gpu or software, got {other:?}")),
            };
        }
        if let Some(t) = &self.theme {
            cfg.theme = find_theme(t).ok_or_else(|| {
                let names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
                anyhow!("theme: no theme called {t:?}; the built-in ones are {}", names.join(", "))
            })?;
        }
        let c = &self.colors;
        if c.ansi.len() > 16 {
            return Err(anyhow!("colors.ansi: at most 16 entries, got {}", c.ansi.len()));
        }
        let ansi = c
            .ansi
            .iter()
            .enumerate()
            .map(|(i, s)| parse_hex(s).ok_or_else(|| anyhow!("colors.ansi[{i}]: expected #rrggbb, got {s:?}")))
            .collect::<Result<Vec<_>>>()?;
        let old = &cfg.colors;
        cfg.colors = Overrides {
            fg: color("foreground", &c.foreground)?.or(old.fg),
            bg: color("background", &c.background)?.or(old.bg),
            cursor: color("cursor", &c.cursor)?.or(old.cursor),
            selection_bg: color("selection_background", &c.selection_background)?.or(old.selection_bg),
            selection_fg: color("selection_foreground", &c.selection_foreground)?.or(old.selection_fg),
            ansi: if ansi.is_empty() { old.ansi.clone() } else { ansi },
        };
        cfg.set_theme(cfg.theme);
        Ok(())
    }
}

/// Reload from defaults so deleting a setting removes its old effect. CLI
/// overrides remain authoritative, and no partial configuration is returned.
pub fn reload(current: &Config) -> Result<Config> {
    let path = current.config_path.as_deref().ok_or_else(|| anyhow!("no configuration path is available"))?;
    let mut next = Config::default();
    load(path)?.apply(&mut next)?;
    current.cli_overrides.apply(&mut next)?;
    next.config_path = current.config_path.clone();
    next.cli_overrides = current.cli_overrides.clone();
    Ok(next)
}
