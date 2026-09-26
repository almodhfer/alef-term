//! Notifications programs ask for: OSC 9, 777 and 99, and the command that
//! puts them on the desktop.
use arabic_term::term::{Notification, Terminal, Vt};

fn feed(input: &str) -> Vec<Notification> {
    let mut t = Terminal::new(2, 20, 10);
    Vt::new().feed(&mut t, input.as_bytes());
    std::mem::take(&mut t.notifications)
}

fn note(title: &str, body: &str) -> Notification {
    Notification { title: title.into(), body: body.into() }
}

#[test]
fn osc_9_is_a_message_even_with_semicolons_in_it() {
    assert_eq!(feed("\x1b]9;Build finished; 3 warnings\x07"), vec![note("", "Build finished; 3 warnings")]);
}

#[test]
fn osc_9_with_a_conemu_command_number_is_not_a_message() {
    // 9;4 is ConEmu's progress report, which other tools emit too.
    assert!(feed("\x1b]9;4;1;50\x1b\\").is_empty());
}

#[test]
fn osc_777_carries_a_title_and_a_body() {
    assert_eq!(
        feed("\x1b]777;notify;Claude Code;Waiting for input; ready\x1b\\"),
        vec![note("Claude Code", "Waiting for input; ready")]
    );
    assert!(feed("\x1b]777;other;x;y\x1b\\").is_empty());
}

#[test]
fn osc_99_joins_chunks_by_id_and_decodes_base64() {
    assert_eq!(
        feed("\x1b]99;i=1:d=0;Hello\x1b\\\x1b]99;i=1:p=body;World\x1b\\"),
        vec![note("Hello", "World")]
    );
    assert_eq!(feed("\x1b]99;e=1;2YXYsdit2KjYpw==\x1b\\"), vec![note("مرحبا", "")]);
    assert!(feed("\x1b]99;i=2:p=?;\x1b\\").is_empty(), "queries are not notifications");
}

#[test]
fn notifications_are_bounded_in_size_and_number() {
    let long = "x".repeat(5000);
    let got = feed(&format!("\x1b]777;notify;{long};{long}\x07"));
    assert!(got[0].title.chars().count() <= 256 && got[0].body.chars().count() <= 1024);
    let flood: String = (0..50).map(|i| format!("\x1b]9;n{i}\x07")).collect();
    assert_eq!(feed(&flood).len(), 8);
}

#[test]
fn the_desktop_command_passes_text_as_arguments_not_code() {
    let cmd = arabic_term::notify::command("-rf \"x\"", "<b>&</b>");
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    if cfg!(target_os = "macos") {
        assert_eq!(cmd.get_program(), "osascript");
        assert_eq!(&args[args.len() - 2..], ["-rf \"x\"", "<b>&</b>"]);
    } else {
        assert_eq!(cmd.get_program(), "notify-send");
        let dash = args.iter().position(|a| a == "--").expect("options end before the text");
        assert_eq!(&args[dash + 1..], ["-rf \"x\"", "&lt;b&gt;&amp;&lt;/b&gt;"]);
    }
}
