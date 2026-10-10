use alef_term::app;
use alef_term::color::THEMES;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        if cfg!(debug_assertions) { "alef_term=debug,warn" } else { "warn" }
    )).init();
    let (cfg, command) = alef_term::cli::parse(std::env::args().skip(1))?;
    match command {
        alef_term::cli::Command::Help => {
            println!(r#"alef-term — a terminal that renders Arabic correctly

Options:
  --font-size <px>              base font size (default 15)
  --arabic-scale <f>            Arabic face scale (default: from metrics)
  --line-height <1..2>          line spacing multiplier (default 1)
  --bidi-mode <auto|ltr|rtl>     text direction (default auto)
  --size <cols>x<rows>          initial grid size (default 96x28)
  --working-directory, -d <path> starting folder
  --exec, -e <cmd>             run a command in the first session
  --scroll-lines <n>           lines per wheel notch (default 3)
  --touch-scroll <f>           trackpad scroll speed (default 3)
  --no-alternate-scroll        disable wheel-to-arrow conversion in full-screen programs
  --theme <name>               colour theme (default dark)
  --list-themes                print the built-in themes
  --renderer <gpu|software>    drawing backend (default gpu)
  --config <path>              settings file (default ~/.config/alef-term/config.toml)
  --check-config               validate settings without opening a window
  --version                    print version
  --help                       show this help

Keys:
  Cmd T                       new tab
  Cmd N                       new window
  Cmd W                       close current tab
  Cmd Shift [ / ]             previous / next tab
  Cmd 1..8 / 9                select tab / last tab
  Cmd Shift P                 command palette
  Cmd Shift R                 reload settings
  Cmd Shift T                 choose a theme
  Cmd Shift F                 choose an Arabic font
  Cmd F                       search scrollback
  Cmd A / C / V               select all / copy / paste
  Cmd K                       clear scrollback
  Cmd +/-/0                   font size / reset to configured size
  Cmd R                       cycle text direction (auto/LTR/RTL)"#);
            return Ok(());
        }
        alef_term::cli::Command::Version => {
            println!("alef-term {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        alef_term::cli::Command::ListThemes => {
            for t in THEMES { println!("{:<18} {}", t.name, t.title); }
            return Ok(());
        }
        alef_term::cli::Command::CheckConfig => {
            println!("Configuration is valid");
            return Ok(());
        }
        alef_term::cli::Command::Run => {}
    }

    let event_loop = EventLoop::<app::Wake>::with_user_event().build()?;
    // Wait for events rather than spinning: the PTY thread wakes us.
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(cfg, proxy)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
