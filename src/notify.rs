//! Desktop notifications, through the platform's own tool: notify-send on
//! Linux and other freedesktop desktops, osascript on macOS. No shell is
//! involved and the text travels as arguments, so nothing a program prints
//! is ever parsed as code.
use std::process::{Command, Stdio};

/// Show a notification without waiting for it. A failure is logged, not
/// reported: the program asked for a courtesy, not a result.
pub fn show(title: &str, body: &str) {
    let mut cmd = command(title, body);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.spawn() {
        // Reap it off the event loop, so it never lingers as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("could not show a notification: {e}"),
    }
}

/// The command that shows a notification on this platform.
pub fn command(title: &str, body: &str) -> Command {
    #[cfg(target_os = "macos")]
    {
        // The script reads both strings from its arguments, so quotes and
        // backslashes in them need no escaping.
        let mut cmd = Command::new("osascript");
        cmd.args([
            "-e", "on run argv",
            "-e", "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e", "end run",
            title, body,
        ]);
        cmd
    }
    #[cfg(not(target_os = "macos"))]
    {
        // `--` keeps a title starting with a dash from reading as an option.
        // Notification servers render a little HTML in the body, so it is
        // escaped; the summary line is plain text.
        let mut cmd = Command::new("notify-send");
        cmd.args([
            "--app-name=Alef",
            "--icon=alef-term",
            "--hint=string:desktop-entry:alef-term",
            "--",
            title,
            &markup_escape(body),
        ]);
        cmd
    }
}

#[cfg(not(target_os = "macos"))]
fn markup_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}
