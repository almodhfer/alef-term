//! The configuration file.
//!
//! Read from `$ARABIC_TERM_CONFIG`, or `~/.config/arabic-term/config.toml`.
//! Every key is optional and the file may be absent; command-line flags
//! override whatever the file says. Unknown keys are an error, so a typo is
//! reported rather than silently ignored.

use crate::app::Config;
use crate::color::{Palette, Rgb};
use crate::term::CursorShape;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct File {
    /// Monospace family, tried before the built-in preference list.
    pub font: Option<String>,
    /// Arabic family, likewise.
    pub arabic_font: Option<String>,
    pub font_size: Option<f32>,
    /// Extra scale on the Arabic face; omitted derives it from the metrics.
    pub arabic_scale: Option<f32>,
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
    /// "dark" or "light"; the base the colours below adjust.
    pub theme: Option<String>,
    pub colors: Colors,
}

/// Colour overrides, as `#rrggbb`.
#[derive(Deserialize, Default, Debug)]
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
    if let Some(p) = std::env::var_os("ARABIC_TERM_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".config/arabic-term/config.toml"))
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

fn color(name: &str, s: &Option<String>, into: &mut Rgb) -> Result<()> {
    if let Some(s) = s {
        *into = parse_hex(s).ok_or_else(|| anyhow!("colors.{name}: expected #rrggbb, got {s:?}"))?;
    }
    Ok(())
}

impl File {
    /// Fold the file into the runtime configuration.
    pub fn apply(&self, cfg: &mut Config) -> Result<()> {
        if self.font.is_some() {
            cfg.font = self.font.clone();
        }
        if self.arabic_font.is_some() {
            cfg.arabic_font = self.arabic_font.clone();
        }
        if let Some(v) = self.font_size {
            if !(4.0..=200.0).contains(&v) {
                return Err(anyhow!("font_size: {v} is out of range"));
            }
            cfg.font_size = v;
        }
        if self.arabic_scale.is_some() {
            cfg.arabic_scale = self.arabic_scale;
        }
        if let Some(v) = self.scrollback {
            cfg.scrollback = v;
        }
        if let Some(v) = self.cols {
            cfg.cols = v.max(2);
        }
        if let Some(v) = self.rows {
            cfg.rows = v.max(1);
        }
        if let Some(v) = self.scroll_lines {
            cfg.wheel.wheel = v;
        }
        if let Some(v) = self.touch_scroll {
            cfg.wheel.touch = v;
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
        if let Some(t) = &self.theme {
            cfg.palette = match t.as_str() {
                "dark" => Palette::dark(),
                "light" => Palette::light(),
                other => return Err(anyhow!("theme: expected dark or light, got {other:?}")),
            };
        }
        let c = &self.colors;
        color("foreground", &c.foreground, &mut cfg.palette.fg)?;
        color("background", &c.background, &mut cfg.palette.bg)?;
        color("cursor", &c.cursor, &mut cfg.palette.cursor)?;
        color("selection_background", &c.selection_background, &mut cfg.palette.selection_bg)?;
        if let Some(s) = &c.selection_foreground {
            cfg.palette.selection_fg =
                Some(parse_hex(s).ok_or_else(|| anyhow!("colors.selection_foreground: expected #rrggbb, got {s:?}"))?);
        }
        if c.ansi.len() > 16 {
            return Err(anyhow!("colors.ansi: at most 16 entries, got {}", c.ansi.len()));
        }
        for (i, s) in c.ansi.iter().enumerate() {
            cfg.palette.colors[i] =
                parse_hex(s).ok_or_else(|| anyhow!("colors.ansi[{i}]: expected #rrggbb, got {s:?}"))?;
        }
        Ok(())
    }
}
