//! The configuration file: parsing, validation and how it lands in Config.
use arabic_term::app::Config;
use arabic_term::color::Rgb;
use arabic_term::config::{parse, parse_hex};
use arabic_term::term::CursorShape;

fn err(text: &str) -> String {
    match apply(text) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("{text:?} was accepted"),
    }
}

fn apply(text: &str) -> anyhow::Result<Config> {
    let mut cfg = Config::default();
    parse(text)?.apply(&mut cfg)?;
    Ok(cfg)
}

#[test]
fn an_empty_file_changes_nothing() {
    let cfg = apply("").unwrap();
    let d = Config::default();
    assert_eq!(cfg.font_size, d.font_size);
    assert_eq!(cfg.palette.bg, d.palette.bg);
}

#[test]
fn the_example_file_parses_and_applies() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/config.example.toml")).unwrap();
    let cfg = apply(&text).unwrap();
    assert_eq!(cfg.cols, 96);
    assert_eq!(cfg.cursor_shape, CursorShape::Block);
}

#[test]
fn values_land_in_the_right_places() {
    let cfg = apply(r##"
font = "Menlo"
font_size = 18
scrollback = 500
scroll_lines = 5
cursor_shape = "bar"
cursor_blink = true
visual_bell = false
theme = "light"
[colors]
background = "#ffffff"
cursor = "ff0000"
ansi = ["#000000", "#111111"]
"##).unwrap();
    assert_eq!(cfg.font.as_deref(), Some("Menlo"));
    assert_eq!(cfg.font_size, 18.0);
    assert_eq!(cfg.scrollback, 500);
    assert_eq!(cfg.wheel.wheel, 5.0);
    assert_eq!(cfg.cursor_shape, CursorShape::Bar);
    assert!(cfg.cursor_blink);
    assert!(!cfg.visual_bell);
    // Light theme, then overrides on top of it.
    assert_eq!(cfg.palette.bg, Rgb(255, 255, 255));
    assert_eq!(cfg.palette.cursor, Rgb(255, 0, 0));
    assert_eq!(cfg.palette.colors[1], Rgb(0x11, 0x11, 0x11));
    assert_ne!(cfg.palette.fg, Config::default().palette.fg, "light theme foreground differs from dark");
}

#[test]
fn mistakes_are_reported_not_ignored() {
    assert!(err("font_sise = 12").contains("font_sise"));
    assert!(err("cursor_shape = \"round\"").contains("cursor_shape"));
    assert!(err("theme = \"solarized\"").contains("theme"));
    assert!(err("[colors]\nbackground = \"red\"").contains("background"));
    assert!(apply("font_size = 1000").is_err());
    assert!(err("renderer = \"vulkan\"").contains("renderer"));
    assert!(apply("renderer = \"software\"").unwrap().software);
}

#[test]
fn hex_colours() {
    assert_eq!(parse_hex("#1a2B3c"), Some(Rgb(0x1a, 0x2b, 0x3c)));
    assert_eq!(parse_hex("1a2b3c"), Some(Rgb(0x1a, 0x2b, 0x3c)));
    assert_eq!(parse_hex("#fff"), None);
    assert_eq!(parse_hex("#gg0000"), None);
}

#[test]
fn typography_and_direction_can_be_configured_without_changing_the_theme() {
    let cfg = apply("line_height = 1.2\npadding_x = 18\npadding_y = 12\nbidi_mode = \"ltr\"").unwrap();
    assert_eq!(cfg.line_height, 1.2);
    assert_eq!(cfg.padding_x, 18.0);
    assert_eq!(cfg.padding_y, 12.0);
    assert_eq!(cfg.bidi_mode, arabic_term::term::BidiMode::ForceLtr);
    assert_eq!(cfg.palette.bg, Config::default().palette.bg);
    assert_eq!(apply("").unwrap().bidi_mode, arabic_term::term::BidiMode::Auto);
    for invalid in ["line_height = 0.5", "line_height = nan", "padding_x = -1",
                    "padding_y = inf", "bidi_mode = \"guess\"", "arabic_scale = nan"] {
        assert!(apply(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn minimum_contrast_is_validated_and_can_be_disabled() {
    let mut cfg = arabic_term::app::Config::default();
    assert_eq!(cfg.minimum_contrast, 4.5);
    arabic_term::config::parse("minimum_contrast = 1").unwrap().apply(&mut cfg).unwrap();
    assert_eq!(cfg.minimum_contrast, 1.0);
    for value in ["0.5", "22", "nan", "inf"] {
        assert!(arabic_term::config::parse(&format!("minimum_contrast = {value}")).unwrap().apply(&mut cfg).is_err());
        assert_eq!(cfg.minimum_contrast, 1.0);
    }
}
