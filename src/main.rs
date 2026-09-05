use arabic_term::app;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let mut cfg = app::Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--font-size" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.font_size = v;
                }
            }
            "--arabic-scale" => {
                cfg.arabic_scale = args.next().and_then(|v| v.parse().ok());
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
                       --exec <cmd>           run a command at startup\n\n\
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
