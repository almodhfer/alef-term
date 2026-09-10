//! The configuration file.
//!
//! Read from `$ARABIC_TERM_CONFIG`, or `~/.config/arabic-term/config.toml`.
//! Every key is optional and the file may be absent; command-line flags
//! override whatever the file says. Unknown keys are an error, so a typo is
//! reported rather than silently ignored.

use crate::app::Config;
use crate::color::{find_theme, Overrides, Rgb, THEMES};
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
    /// A built-in theme, by name (`arabic-term --list-themes`); the base the
    /// colours below adjust.
    pub theme: Option<String>,
    /// "gpu" (the default) or "software".
    pub renderer: Option<String>,
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

fn color(name: &str, s: &Option<String>) -> Result<Option<Rgb>> {
    s.as_ref()
        .map(|s| parse_hex(s).ok_or_else(|| anyhow!("colors.{name}: expected #rrggbb, got {s:?}")))
        .transpose()
}

/// Record `theme` as the configured theme and change nothing else in the
/// file. A file that does not parse is left alone rather than clobbered; one
/// that does not exist is created.
pub fn save_theme(path: &Path, theme: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    parse(&text).with_context(|| format!("{} has an error; not saving the theme", path.display()))?;
    let updated = set_theme_line(&text, theme);
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
    let line = format!("theme = \"{theme}\"");
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for l in text.lines() {
        let t = l.trim_start();
        let is_theme = t.strip_prefix("theme").is_some_and(|rest| rest.trim_start().starts_with('='));
        if !done && is_theme {
            // Theme names hold no `#`, so the first one starts the comment.
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
        cfg.colors = Overrides {
            fg: color("foreground", &c.foreground)?,
            bg: color("background", &c.background)?,
            cursor: color("cursor", &c.cursor)?,
            selection_bg: color("selection_background", &c.selection_background)?,
            selection_fg: color("selection_foreground", &c.selection_foreground)?,
            ansi,
        };
        cfg.set_theme(cfg.theme);
        Ok(())
    }
}
