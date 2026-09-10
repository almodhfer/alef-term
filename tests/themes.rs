//! Built-in themes, the picker's filtering, and remembering a choice.
use arabic_term::app::Config;
use arabic_term::color::{find_theme, Palette, Rgb, Theme, THEMES};
use arabic_term::config::{parse, save_theme, set_theme_line};
use arabic_term::picker::ThemePicker;

fn theme(name: &str) -> &'static Theme {
    find_theme(name).unwrap_or_else(|| panic!("no theme {name}"))
}

fn luminance(c: Rgb) -> f64 {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[test]
fn names_are_unique_and_found_every_way_they_are_written() {
    for (i, t) in THEMES.iter().enumerate() {
        assert!(THEMES[..i].iter().all(|o| o.name != t.name && o.title != t.title), "{} twice", t.name);
        assert!(t.name.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{} is not kebab-case", t.name);
        assert!(std::ptr::eq(theme(t.name), t));
        assert!(std::ptr::eq(theme(t.title), t));
    }
    assert!(std::ptr::eq(theme("Tokyo Night"), theme("tokyo-night")));
    assert!(std::ptr::eq(theme("TOKYO_NIGHT"), theme("tokyo-night")));
    assert!(std::ptr::eq(theme("rosé pine"), theme("rose-pine")));
    assert!(find_theme("solarized").is_none());
    assert!(THEMES.len() >= 18);
}

#[test]
fn every_theme_has_readable_text() {
    for t in THEMES {
        assert!(contrast(t.fg, t.bg) >= 4.0, "{}: foreground on background is {:.1}:1", t.name, contrast(t.fg, t.bg));
        assert!(contrast(t.cursor, t.bg) >= 1.5, "{}: the cursor disappears", t.name);
        assert_ne!(t.selection_bg, t.bg, "{}: selection is invisible", t.name);
        assert_ne!(t.search_bg, t.search_current_bg, "{}: the current hit looks like the others", t.name);
    }
}

#[test]
fn the_original_palettes_are_unchanged() {
    assert_eq!(Palette::dark().bg, Rgb(0x14, 0x16, 0x1b));
    assert_eq!(Palette::dark().colors[1], Rgb(0xe0, 0x60, 0x6a));
    assert_eq!(Palette::light().bg, Rgb(0xfa, 0xfa, 0xf7));
    assert_eq!(Palette::light().search_current_bg, Rgb(0xff, 0xb3, 0x40));
    // The 256-colour cube is the same whatever the theme.
    assert_eq!(Palette::from_theme(theme("nord")).colors[196], Rgb(255, 0, 0));
}

#[test]
fn the_picker_filters_and_keeps_its_place() {
    let mut p = ThemePicker::new(theme("nord"));
    assert!(std::ptr::eq(p.current().unwrap(), theme("nord")), "opens on the theme in use");

    p.push("gruv");
    let names: Vec<_> = p.matches().iter().map(|t| t.name).collect();
    assert_eq!(names, ["gruvbox-dark", "gruvbox-light"]);
    p.step(1);
    assert_eq!(p.current().unwrap().name, "gruvbox-light");
    p.step(1);
    assert_eq!(p.current().unwrap().name, "gruvbox-dark", "wraps around");
    p.step(-1);

    // Narrowing keeps the highlight on the same theme while it still matches.
    p.push("box l");
    assert_eq!(p.current().unwrap().name, "gruvbox-light");
    p.pop(true);
    assert_eq!(p.current().unwrap().name, "gruvbox-light");
    assert_eq!(p.matches().len(), THEMES.len());

    p.push("no such theme");
    assert!(p.current().is_none());
    p.step(1);
    p.pop(false);
    assert!(p.current().is_none());
    p.pop(true);
    assert!(p.current().is_some());

    // Matched on the name as well as the title.
    let mut p = ThemePicker::new(theme("dark"));
    p.push("rose");
    assert_eq!(p.current().unwrap().name, "rose-pine");
}

#[test]
fn any_theme_can_be_configured_and_overrides_follow_a_switch() {
    let mut cfg = Config::default();
    parse("theme = \"Tokyo Night\"\n[colors]\ncursor = \"#ff0000\"\nansi = [\"#000000\", \"#111111\"]\n")
        .unwrap()
        .apply(&mut cfg)
        .unwrap();
    assert_eq!(cfg.theme.name, "tokyo-night");
    assert_eq!(cfg.palette.bg, theme("tokyo-night").bg);
    assert_eq!(cfg.palette.cursor, Rgb(255, 0, 0));

    // Picking another theme at run time keeps the file's overrides.
    cfg.set_theme(theme("nord"));
    assert_eq!(cfg.palette.bg, theme("nord").bg);
    assert_eq!(cfg.palette.fg, theme("nord").fg);
    assert_eq!(cfg.palette.cursor, Rgb(255, 0, 0));
    assert_eq!(cfg.palette.colors[1], Rgb(0x11, 0x11, 0x11));
    assert_eq!(cfg.palette.colors[2], theme("nord").ansi[2]);

    let mut cfg = Config::default();
    let e = parse("theme = \"solarized\"").unwrap().apply(&mut cfg).unwrap_err().to_string();
    assert!(e.contains("solarized-dark"), "the error lists the real names: {e}");
}

#[test]
fn the_theme_line_is_rewritten_in_place() {
    let text = "font_size = 15\ntheme = \"dark\"              # a comment\n\n[colors]\ncursor = \"#ff0000\"\n";
    let out = set_theme_line(text, "nord");
    assert_eq!(out, "font_size = 15\ntheme = \"nord\"              # a comment\n\n[colors]\ncursor = \"#ff0000\"\n");
    // A name longer than the gap still leaves a space before the comment.
    let out = set_theme_line("theme = \"x\" # c\n", "catppuccin-mocha");
    assert_eq!(out, "theme = \"catppuccin-mocha\" # c\n");
}

#[test]
fn a_missing_theme_line_goes_above_the_tables() {
    assert_eq!(set_theme_line("", "nord"), "theme = \"nord\"\n");
    assert_eq!(set_theme_line("font_size = 15", "nord"), "font_size = 15\ntheme = \"nord\"\n");
    let out = set_theme_line("font_size = 15\n[colors]\ncursor = \"#ff0000\"\n", "nord");
    assert_eq!(out, "font_size = 15\ntheme = \"nord\"\n\n[colors]\ncursor = \"#ff0000\"\n");
    let mut cfg = Config::default();
    parse(&out).unwrap().apply(&mut cfg).unwrap();
    assert_eq!(cfg.theme.name, "nord");
}

#[test]
fn the_example_file_takes_a_saved_theme() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/config.example.toml")).unwrap();
    let out = set_theme_line(&text, "kanagawa");
    assert_eq!(out.lines().count(), text.lines().count(), "one line changed, none added");
    let mut cfg = Config::default();
    parse(&out).unwrap().apply(&mut cfg).unwrap();
    assert_eq!(cfg.theme.name, "kanagawa");
    assert_eq!(cfg.cols, 96, "the rest of the file is intact");
}

#[test]
fn saving_creates_the_file_and_never_clobbers_a_broken_one() {
    let dir = std::env::temp_dir().join(format!("arabic-term-themes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested/config.toml");

    save_theme(&path, "dracula").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"dracula\"\n");
    save_theme(&path, "nord").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"nord\"\n");

    std::fs::write(&path, "font_sise = 12\n").unwrap();
    assert!(save_theme(&path, "dracula").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "font_sise = 12\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_picker_is_painted_in_each_themes_colours() {
    use arabic_term::font::Fonts;
    use arabic_term::render::{Canvas, DrawOpts, PickerView, Renderer};
    use arabic_term::term::Terminal;

    let t = Terminal::new(34, 70, 100);
    let mut fonts = Fonts::new(15.0, None).unwrap();
    let mut renderer = Renderer::new();
    let w = (70.0 * fonts.cell_w) as usize + 8;
    let h = (34.0 * fonts.cell_h) as usize + 4;
    let mut buf = vec![0u32; w * h];
    let mut canvas = Canvas { buf: &mut buf, width: w, height: h };
    let themes: Vec<&'static Theme> = THEMES.iter().collect();
    let opts = DrawOpts {
        picker: Some(PickerView { query: "", themes: &themes, selected: 3 }),
        ..Default::default()
    };
    renderer.draw(&mut canvas, &t, &mut fonts, &opts);

    let count = |c: Rgb| buf.iter().filter(|&&p| p == c.pack()).count();
    for th in THEMES {
        assert!(count(th.bg) > 200, "{} has its row", th.name);
    }
    assert!(count(theme("dracula").ansi[5]) > 20, "swatches are drawn");
    assert!(count(t.palette.cursor) > 200, "the frame and the ring round the highlight");
}
