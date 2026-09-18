//! Validate invocation before opening a window or allocating the terminal.
use crate::{app::Config, config::File};
use anyhow::{anyhow, Context, Result};

#[derive(Debug, PartialEq, Eq)]
pub enum Command { Run, Help, Version, ListThemes, CheckConfig }

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<(Config, Command)> {
    let mut args = args.into_iter();
    let mut file = File::default();
    let mut path = crate::config::default_path();
    let mut startup = None;
    let mut command = Command::Run;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow!("{arg}: missing value"));
        match arg.as_str() {
            "--help" | "-h" => command = Command::Help,
            "--version" | "-V" => command = Command::Version,
            "--list-themes" => command = Command::ListThemes,
            "--check-config" => command = Command::CheckConfig,
            "--config" => path = Some(value()?.into()),
            "--font-size" => file.font_size = Some(value()?.parse().context("--font-size: expected a number")?),
            "--arabic-scale" => file.arabic_scale = Some(value()?.parse().context("--arabic-scale: expected a number")?),
            "--line-height" => file.line_height = Some(value()?.parse().context("--line-height: expected a number")?),
            "--scroll-lines" => file.scroll_lines = Some(value()?.parse().context("--scroll-lines: expected a number")?),
            "--touch-scroll" => file.touch_scroll = Some(value()?.parse().context("--touch-scroll: expected a number")?),
            "--no-alternate-scroll" => file.alternate_scroll = Some(false),
            "--theme" => file.theme = Some(value()?),
            "--renderer" => file.renderer = Some(value()?),
            "--bidi-mode" => file.bidi_mode = Some(value()?),
            "--working-directory" | "-d" => file.working_directory = Some(value()?),
            "--exec" | "-e" => startup = Some(format!("{}\n", value()?)),
            "--size" => {
                let size = value()?;
                let (cols, rows) = size.split_once('x').ok_or_else(|| anyhow!("--size: expected COLSxROWS"))?;
                file.cols = Some(cols.parse().context("--size: invalid column count")?);
                file.rows = Some(rows.parse().context("--size: invalid row count")?);
            }
            _ => return Err(anyhow!("unknown option {arg:?}; use --help")),
        }
    }
    let mut cfg = Config::default();
    if matches!(command, Command::Help | Command::Version | Command::ListThemes) {
        return Ok((cfg, command));
    }
    if let Some(path) = &path {
        if let Err(error) = crate::config::load(path).and_then(|file| file.apply(&mut cfg)) {
            if command == Command::CheckConfig { return Err(error); }
            log::warn!("configuration: {error:#}; using defaults");
            cfg.startup_warning = Some(format!("Settings could not be loaded; using defaults. {error}"));
        }
    }
    file.apply(&mut cfg)?;
    cfg.cli_overrides = file;
    cfg.config_path = path;
    cfg.startup_cmd = startup;
    Ok((cfg, command))
}
