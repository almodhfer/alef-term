//! Synchronized output (DEC mode 2026): holding frames during an update.

use arabic_term::frame_hold::{FrameHold, TIMEOUT};
use arabic_term::term::{Modes, Terminal, Vt};
use std::time::{Duration, Instant};

fn t0() -> Instant {
    Instant::now()
}

#[test]
fn the_mode_is_off_until_an_application_asks_for_it() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    assert!(!t.modes.contains(Modes::SYNC_OUTPUT), "must not be on by default");
    vt.feed(&mut t, b"\x1b[?2026h");
    assert!(t.modes.contains(Modes::SYNC_OUTPUT));
    vt.feed(&mut t, b"\x1b[?2026l");
    assert!(!t.modes.contains(Modes::SYNC_OUTPUT));
}

#[test]
fn damage_during_a_hold_is_withheld_then_released() {
    let mut h = FrameHold::new();
    let now = t0();

    // Update begins; content changes but nothing is presented.
    assert_eq!(h.poll(true, true, now).redraw, false, "held");
    assert!(h.holding());
    assert_eq!(h.poll(true, true, now + Duration::from_millis(10)).redraw, false);

    // Update ends: exactly one frame is owed.
    let out = h.poll(false, false, now + Duration::from_millis(20));
    assert!(out.redraw, "the accumulated update is presented");
    assert!(!out.drop_mode);
    assert!(!h.holding());

    // And it is not presented twice.
    assert_eq!(h.poll(false, false, now + Duration::from_millis(30)).redraw, false);
}

#[test]
fn an_update_that_changed_nothing_costs_no_frame() {
    let mut h = FrameHold::new();
    let now = t0();
    h.poll(true, false, now);
    let out = h.poll(false, false, now + Duration::from_millis(5));
    assert!(!out.redraw, "nothing changed, so nothing to present");
}

#[test]
fn the_watchdog_breaks_a_hold_nothing_ever_closes() {
    // A program that begins an update and then crashes never sends the end
    // sequence. Without this the display would freeze permanently.
    let mut h = FrameHold::new();
    let now = t0();
    assert!(!h.poll(true, true, now).redraw);

    let out = h.poll(true, false, now + TIMEOUT);
    assert!(out.redraw, "the withheld frame is finally shown");
    assert!(out.drop_mode, "and the mode is abandoned");
    assert!(!h.holding());
}

#[test]
fn the_watchdog_does_not_fire_early() {
    let mut h = FrameHold::new();
    let now = t0();
    h.poll(true, true, now);
    let out = h.poll(true, false, now + TIMEOUT - Duration::from_millis(1));
    assert!(!out.redraw);
    assert!(!out.drop_mode);
}

#[test]
fn a_deadline_is_exposed_only_while_holding() {
    let mut h = FrameHold::new();
    let now = t0();
    assert!(h.deadline().is_none(), "nothing to wake for when idle");
    h.poll(true, true, now);
    assert_eq!(h.deadline(), Some(now + TIMEOUT), "the loop can wake itself");
    h.poll(false, false, now + Duration::from_millis(1));
    assert!(h.deadline().is_none());
}

#[test]
fn ordinary_damage_still_draws_when_the_mode_is_off() {
    let mut h = FrameHold::new();
    let now = t0();
    assert!(h.poll(false, true, now).redraw);
    assert!(!h.poll(false, false, now).redraw);
}

#[test]
fn an_update_bracketed_inside_one_write_holds_nothing() {
    // Programs commonly emit BSU, the whole repaint, and ESU in a single
    // write. By the time the batch is parsed the mode is already off, so the
    // frame must be presented immediately rather than held for the watchdog.
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[?2026h hello \x1b[?2026l");
    assert!(!t.modes.contains(Modes::SYNC_OUTPUT), "mode already closed");

    let mut h = FrameHold::new();
    let out = h.poll(t.modes.contains(Modes::SYNC_OUTPUT), t.damaged, t0());
    assert!(out.redraw, "must present at once, not wait 150ms");
    assert!(!h.holding());
}

#[test]
fn a_second_update_starts_a_fresh_watchdog() {
    let mut h = FrameHold::new();
    let now = t0();
    h.poll(true, true, now);
    h.poll(false, false, now + Duration::from_millis(10));
    // Second update begins well after the first would have timed out.
    let later = now + Duration::from_secs(1);
    h.poll(true, true, later);
    assert_eq!(h.deadline(), Some(later + TIMEOUT), "timer restarts");
    assert!(!h.poll(true, false, later + Duration::from_millis(10)).redraw);
}

#[test]
fn decrqm_reports_synchronized_output_as_supported() {
    // Applications query before using the feature; an unanswered query means
    // "unsupported" and the mode is never sent.
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[?2026$p");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?2026;2$y", "known, currently reset");

    t.replies.clear();
    vt.feed(&mut t, b"\x1b[?2026h\x1b[?2026$p");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?2026;1$y", "known, currently set");
}

#[test]
fn decrqm_distinguishes_unknown_modes_from_reset_ones() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    // 2004 is real and off; 9999 is not something we implement.
    vt.feed(&mut t, b"\x1b[?2004$p\x1b[?9999$p");
    assert_eq!(
        String::from_utf8_lossy(&t.replies),
        "\x1b[?2004;2$y\x1b[?9999;0$y",
        "2 = reset, 0 = unrecognised"
    );
}

#[test]
fn decrqm_reports_the_alternate_screen() {
    let mut t = Terminal::new(4, 20, 100);
    let mut vt = Vt::new();
    vt.feed(&mut t, b"\x1b[?1049h\x1b[?1049$p");
    assert_eq!(String::from_utf8_lossy(&t.replies), "\x1b[?1049;1$y");
}
