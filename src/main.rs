use arabic_term::app;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let mut cfg = app::Config::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // The file first, so that flags can override it.
    let config_path = argv
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| argv.get(i + 1).map(std::path::PathBuf::from))
        .or_else(arabic_term::config::default_path);
    if let Some(path) = &config_path {
        match arabic_term::config::load(path).and_then(|f| f.apply(&mut cfg)) {
            Ok(()) => {}
            Err(e) => eprintln!("arabic-term: config: {e:#}; continuing with defaults"),
        }
    }
    let mut args = argv.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                args.next();
            }
            "--font-size" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.font_size = v;
                }
            }
            "--arabic-scale" => {
                cfg.arabic_scale = args.next().and_then(|v| v.parse().ok());
            }
            "--scroll-lines" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.wheel.wheel = v;
                }
            }
            "--touch-scroll" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.wheel.touch = v;
                }
            }
            "--no-alternate-scroll" => cfg.alternate_scroll = false,
            "--size" => {
                if let Some((c, r)) = args.next().and_then(|v| {
                    let (c, r) = v.split_once('x')?;
                    Some((c.parse().ok()?, r.parse().ok()?))
                }) {
                    cfg.cols = c;
                    cfg.rows = r;
                }
            }
            "--exec" | "-e" => {
                if let Some(v) = args.next() {
                    cfg.startup_cmd = Some(format!("{v}\n"));
                }
            }
            "--help" | "-h" => {
                println!(
                    "arabic-term — a terminal that renders Arabic correctly\n\n\
                     Options:\n  \
                       --font-size <px>       base font size (default 15)\n  \
                       --arabic-scale <f>     Arabic face scale (default: from metrics)\n  \
                       --exec <cmd>           run a command at startup\n  \
                       --size <cols>x<rows>   initial grid size (default 96x28)\n  \
                       --scroll-lines <n>     lines per wheel notch (default 3)\n  \
                       --touch-scroll <f>     trackpad scroll speed (default 3)\n  \
                       --no-alternate-scroll  do not turn the wheel into arrow\n  \
                       \u{20}                      keys for full-screen programs\n  \
                       --config <path>        configuration file (default:\n  \
                       \u{20}                      ~/.config/arabic-term/config.toml)\n\n\
                     Keys:\n  \
                       Cmd +/-/0    font size\n  \
                       Cmd R        cycle bidi base direction (auto/LTR/RTL)\n  \
                       Cmd C/V      copy / paste"
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let event_loop = EventLoop::<app::Wake>::with_user_event().build()?;
    // Wait for events rather than spinning: the PTY thread wakes us.
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(cfg, proxy)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
