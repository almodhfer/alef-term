//! Scroll easing, coalescing, and the grid's absolute line indexing.

use arabic_term::scroll::Scroller;
use arabic_term::term::{Terminal, Vt};

const FRAME: f32 = 1.0 / 60.0;

#[test]
fn a_nudge_is_not_applied_until_the_next_frame() {
    // Coalescing: events accumulate and the frame applies them once.
    let mut s = Scroller::default();
    s.nudge(3.0);
    s.nudge(3.0);
    s.nudge(3.0);
    assert_eq!(s.pos, 0.0, "nothing moves until a frame runs");
    s.step(FRAME, 100.0);
    assert_eq!(s.target, 9.0, "all three ticks land in one step");
}

#[test]
fn easing_converges_and_then_reports_finished() {
    let mut s = Scroller::default();
    s.nudge(10.0);
    let mut frames = 0;
    while s.step(FRAME, 100.0) {
        frames += 1;
        assert!(frames < 600, "easing must terminate");
    }
    assert!((s.pos - 10.0).abs() < 0.02, "settles on the target, got {}", s.pos);
    assert!(frames > 1, "movement is eased, not instant");
    assert!(frames < 120, "settles within about two seconds, took {frames} frames");
}

#[test]
fn easing_is_framerate_independent() {
    // The same wall-clock time should cover the same distance whether the
    // display runs at 60 or 120 Hz.
    let mut slow = Scroller::default();
    let mut fast = Scroller::default();
    slow.nudge(20.0);
    fast.nudge(20.0);
    for _ in 0..6 {
        slow.step(1.0 / 60.0, 100.0);
    }
    for _ in 0..12 {
        fast.step(1.0 / 120.0, 100.0);
    }
    assert!(
        (slow.pos - fast.pos).abs() < 0.05,
        "60Hz reached {} but 120Hz reached {}",
        slow.pos,
        fast.pos
    );
}

#[test]
fn scrolling_is_clamped_to_the_scrollback() {
    let mut s = Scroller::default();
    s.nudge(1000.0);
    s.step(FRAME, 25.0);
    assert_eq!(s.target, 25.0, "cannot scroll past the oldest line");
    s.nudge(-1000.0);
    s.step(FRAME, 25.0);
    assert_eq!(s.target, 0.0, "cannot scroll past the live screen");
}

#[test]
fn offset_and_fraction_split_the_position() {
    let mut s = Scroller::default();
    s.jump_to(3.25, 100.0);
    assert_eq!(s.offset(), 3);
    assert!((s.frac() - 0.25).abs() < 1e-6);
}

#[test]
fn snapping_to_the_bottom_cancels_movement_in_flight() {
    let mut s = Scroller::default();
    s.nudge(40.0);
    s.step(FRAME, 100.0);
    assert!(s.pos > 0.0);
    s.snap_to_bottom();
    assert!(s.at_bottom());
    assert!(!s.step(FRAME, 100.0), "nothing left to animate");
    assert_eq!(s.pos, 0.0);
}

#[test]
fn grid_indexes_scrollback_and_screen_as_one_sequence() {
    let mut t = Terminal::new(3, 20, 100);
    let mut vt = Vt::new();
    for i in 1..=8 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    // 8 lines written into a 3-row screen: 6 retired to scrollback.
    assert_eq!(t.grid.scrollback.len(), 6);
    assert_eq!(t.grid.total_lines(), 9);

    let text = |abs: isize| -> String {
        t.grid
            .line(abs)
            .map(|r| r.cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string())
            .unwrap_or_default()
    };
    assert_eq!(text(0), "line1", "oldest scrollback line");
    assert_eq!(text(5), "line6");
    assert_eq!(text(6), "line7", "first screen row continues the sequence");
    assert_eq!(t.grid.line(-1).is_none(), true, "before the beginning");
    assert_eq!(t.grid.line(99).is_none(), true, "past the end");
}

#[test]
fn viewport_top_moves_with_the_offset() {
    let mut t = Terminal::new(3, 20, 100);
    let mut vt = Vt::new();
    for i in 1..=8 {
        vt.feed(&mut t, format!("line{i}\r\n").as_bytes());
    }
    assert_eq!(t.grid.viewport_top(), 6, "live screen starts after the scrollback");
    t.grid.view_offset = 4;
    assert_eq!(t.grid.viewport_top(), 2);
    let first = t.grid.viewport_row(0);
    assert_eq!(
        first.cells.iter().map(|c| c.ch).collect::<String>().trim_end(),
        "line3"
    );
}
