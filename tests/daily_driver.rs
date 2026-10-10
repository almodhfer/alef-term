use alef_term::{app::Config, cli, commands::{Action, CommandPalette}, config, input, pty};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use winit::keyboard::{Key, KeyCode, PhysicalKey};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("alef-term-daily-{}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self, text: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

#[test]
fn arabic_keyboard_keeps_command_shortcuts_and_ascii_layouts_keep_their_bindings() {
    assert_eq!(input::command_key(&Key::Character("ؤ".into()), PhysicalKey::Code(KeyCode::KeyC)), Key::Character("c".into()));
    assert_eq!(input::command_key(&Key::Character("لا".into()), PhysicalKey::Code(KeyCode::KeyV)), Key::Character("v".into()));
    assert_eq!(input::command_key(&Key::Character("Z".into()), PhysicalKey::Code(KeyCode::KeyY)), Key::Character("z".into()));
    assert_eq!(input::command_key(&Key::Character("ز".into()), PhysicalKey::Code(KeyCode::Digit0)), Key::Character("0".into()));
}

#[test]
fn paste_normalizes_cr_lf_and_crlf_exactly_once() {
    assert_eq!(input::paste_bytes("one\r\ntwo\rthree\nfour\tend", false), b"one\rtwo\rthree\rfour\tend");
    assert_eq!(input::paste_bytes("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
    assert_eq!(input::paste_bytes("", true), b"");
}

#[test]
fn paste_preserves_arabic_marks_and_joiners_but_cannot_escape_its_brackets() {
    let text = "بِسْمِ\u{200d} الله\u{200c} English";
    assert_eq!(input::paste_bytes(text, false), text.as_bytes());
    let bytes = input::paste_bytes("a\x1b[201~\x03\u{202e}b\u{202c}\u{009b}", true);
    assert_eq!(bytes, b"\x1b[200~a[201~b\x1b[201~");
}

#[test]
fn dropped_paths_arrive_as_one_literal_shell_argument() {
    let temp = Temp::new();
    let path = temp.0.join("تقرير 'final' $(printf injected).txt");
    let quoted = input::quoted_path(&path).unwrap();
    let output = std::process::Command::new("/bin/sh").args(["-c", &format!("printf '%s' {quoted}")]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, path.to_str().unwrap().as_bytes());
    assert!(input::quoted_path(Path::new("/tmp/bad\npath")).is_none());
    assert!(input::quoted_path(Path::new("/tmp/\u{202e}bad")).is_none());
}

#[test]
fn palette_filters_multiple_words_and_never_executes_a_stale_selection() {
    let mut palette = CommandPalette::default();
    palette.step(-1);
    assert_eq!(palette.current(), Some(Action::DirectionRtl));
    palette.push("settings reload");
    assert_eq!(palette.current(), Some(Action::Reload));
    palette.push(" no match");
    palette.step(1);
    assert_eq!(palette.current(), None);
    palette.backspace(true);
    assert_eq!(palette.current(), Some(Action::NewTab));
}

#[test]
fn invalid_settings_do_not_partially_apply() {
    let mut cfg = Config::default();
    let size = cfg.font_size;
    let bg = cfg.palette.bg;
    let file = config::parse("font_size = 40\npadding_x = 60\n[colors]\nbackground = 'invalid'").unwrap();
    assert!(file.apply(&mut cfg).is_err());
    assert_eq!(cfg.font_size, size);
    assert_eq!(cfg.padding_x, 12.0);
    assert_eq!(cfg.palette.bg, bg);
}

#[test]
fn reload_resets_removed_settings_and_preserves_cli_overrides_and_colors() {
    let temp = Temp::new();
    let path = temp.config("font_size = 24\npadding_x = 30\n[colors]\nbackground = '#123456'");
    let (cfg, _) = cli::parse(vec!["--config".into(), path.to_str().unwrap().into(), "--font-size".into(), "19".into()]).unwrap();
    assert_eq!(cfg.font_size, 19.0);
    assert_eq!(cfg.padding_x, 30.0);
    assert_eq!(cfg.palette.bg, alef_term::color::Rgb(0x12, 0x34, 0x56));
    temp.config("font_size = 20\n");
    let reloaded = config::reload(&cfg).unwrap();
    assert_eq!(reloaded.font_size, 19.0);
    assert_eq!(reloaded.padding_x, Config::default().padding_x);
    assert_eq!(reloaded.palette.bg, Config::default().palette.bg);
    assert!(reloaded.startup_cmd.is_none());
    temp.config("font_size = nan");
    assert!(config::reload(&reloaded).is_err());
    assert_eq!(reloaded.font_size, 19.0);
}

#[test]
fn invalid_cli_values_fail_before_opening_a_window() {
    let temp = Temp::new();
    let path = temp.config("");
    for args in [vec!["--size", "0x24"], vec!["--font-size", "nan"], vec!["--scroll-lines", "inf"],
                 vec!["--size", "999999x99"], vec!["--theme", "missing-theme"],
                 vec!["--font-size"], vec!["--working-directory", "/no/such/folder/alef-term"],
                 vec!["--typo"]] {
        let mut argv = vec!["--config".into(), path.to_str().unwrap().into()];
        argv.extend(args.iter().map(|s| s.to_string()));
        assert!(cli::parse(argv).is_err(), "{args:?}");
    }
}

#[test]
fn help_and_version_work_even_with_broken_configuration() {
    let temp = Temp::new();
    let path = temp.config("broken = [");
    for (arg, expected) in [("--help", cli::Command::Help), ("--version", cli::Command::Version)] {
        assert_eq!(cli::parse(vec!["--config".into(), path.to_str().unwrap().into(), arg.into()]).unwrap().1, expected);
    }
}

#[test]
fn broken_settings_leave_a_usable_shell_but_fail_explicit_validation() {
    let temp = Temp::new();
    let path = temp.config("font_size = 40\npadding_x = -1");
    let args = vec!["--config".into(), path.to_str().unwrap().into()];
    let (cfg, command) = cli::parse(args.clone()).unwrap();
    assert_eq!(command, cli::Command::Run);
    assert_eq!(cfg.font_size, Config::default().font_size);
    assert!(cfg.startup_warning.is_some());
    assert!(cli::parse(args.into_iter().chain(["--check-config".into()])).is_err());
}

#[test]
fn initial_directory_prefers_explicit_then_shell_then_home_for_dock_launches() {
    let home = Temp::new();
    let work = Temp::new();
    assert_eq!(pty::start_directory(None, Some(Path::new("/")), Some(&home.0)), Some(home.0.clone()));
    assert_eq!(pty::start_directory(None, Some(&work.0), Some(&home.0)), Some(work.0.clone()));
    assert_eq!(pty::start_directory(Some(&home.0), Some(&work.0), None), Some(home.0.clone()));
}

#[test]
fn clipboard_selection_is_opt_in() {
    let mut cfg = Config::default();
    assert!(!cfg.copy_on_select);
    config::parse("copy_on_select = true").unwrap().apply(&mut cfg).unwrap();
    assert!(cfg.copy_on_select);
}

#[test]
fn locale_fallback_preserves_explicit_language_preferences() {
    use portable_pty::CommandBuilder;
    let mut cmd = CommandBuilder::new("/bin/sh");
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] { cmd.env_remove(key); }
    pty::configure_locale(&mut cmd);
    assert!(cmd.get_env("LC_CTYPE").unwrap().to_string_lossy().contains("UTF-8"));
    cmd.env("LC_ALL", "ar_SA.UTF-8");
    pty::configure_locale(&mut cmd);
    assert_eq!(cmd.get_env("LC_ALL").unwrap(), "ar_SA.UTF-8");
    cmd.env("LC_ALL", "C");
    pty::configure_locale(&mut cmd);
    assert_eq!(cmd.get_env("LC_ALL").unwrap(), "C");
    if cfg!(target_os = "macos") {
        cmd.env("LC_ALL", "C.UTF-8");
        pty::configure_locale(&mut cmd);
        assert_eq!(cmd.get_env("LC_ALL").unwrap(), "en_US.UTF-8");
    }
}

#[test]
fn arabic_font_commands_are_discoverable_by_style_and_family() {
    let mut palette = CommandPalette::default();
    palette.push("arabic font");
    // Geeza Pro is offered only on macOS, which ships it.
    assert_eq!(palette.matches().len(), if cfg!(target_os = "macos") { 4 } else { 3 });
    palette.backspace(true);
    palette.push("calligraphic");
    assert_eq!(palette.current(), Some(Action::ArabicFont(alef_term::font::ArabicFont::Scheherazade)));
    palette.backspace(true);
    palette.push("amiri");
    assert_eq!(palette.current(), Some(Action::ArabicFont(alef_term::font::ArabicFont::Amiri)));
}

#[test]
fn saving_an_arabic_font_preserves_theme_colors_comments_and_broken_files() {
    let temp = Temp::new();
    let path = temp.config("theme = 'light'\narabic_font = 'Geeza Pro' # Arabic choice\n[colors]\nbackground = '#fafaf7'\n");
    config::save_arabic_font(&path, "Amiri").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# Arabic choice"));
    let file = config::load(&path).unwrap();
    assert_eq!(file.arabic_font.as_deref(), Some("Amiri"));
    assert_eq!(file.theme.as_deref(), Some("light"));
    assert_eq!(file.colors.background.as_deref(), Some("#fafaf7"));
    temp.config("[colors]\nforeground = '#123456'\n");
    config::save_arabic_font(&path, "Noto Naskh Arabic").unwrap();
    assert_eq!(config::load(&path).unwrap().arabic_font.as_deref(), Some("Noto Naskh Arabic"));
    temp.config("broken = [");
    assert!(config::save_arabic_font(&path, "Amiri").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken = [");
}
