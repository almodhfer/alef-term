//! Scroll position and its easing.
//!
//! Kept separate from the window so the motion can be tested directly.

/// How far a wheel or trackpad gesture moves the view.
///
/// Two separate factors, because the inputs are not alike. A wheel reports
/// discrete notches, and one notch moving one line feels stuck — every terminal
/// multiplies it. A trackpad reports pixels, which convert to lines directly;
/// but a 1:1 mapping of finger travel to content ties scrolling to the physical
/// size of a cell, so on a dense display a full swipe barely moves the page.
#[derive(Clone, Copy, Debug)]
pub struct WheelTuning {
    /// Lines per notch of a discrete wheel.
    pub wheel: f32,
    /// Multiplier on the pixels-to-lines conversion for continuous devices.
    pub touch: f32,
}

impl Default for WheelTuning {
    fn default() -> Self {
        WheelTuning { wheel: 3.0, touch: 3.0 }
    }
}

/// A scroll delta as the windowing layer reports it.
#[derive(Clone, Copy, Debug)]
pub enum WheelDelta {
    /// Discrete notches, positive scrolling back into history.
    Lines(f32),
    /// Continuous pixels, positive scrolling back into history.
    Pixels(f32),
}

/// Convert a raw delta into lines of movement.
///
/// When an application has grabbed the mouse the multipliers are dropped: a
/// report should say how far the user actually scrolled, and inflating it
/// claims movement that did not happen.
pub fn wheel_to_lines(
    delta: WheelDelta,
    cell_h: f32,
    tuning: WheelTuning,
    grabbed: bool,
) -> f32 {
    match delta {
        WheelDelta::Lines(y) => {
            if grabbed {
                y
            } else {
                y * tuning.wheel
            }
        }
        WheelDelta::Pixels(py) => {
            let lines = py / cell_h.max(1.0);
            if grabbed {
                lines
            } else {
                lines * tuning.touch
            }
        }
    }
}

/// Tracks where the view is, where it is heading, and how fast it gets there.
#[derive(Default, Debug, Clone, Copy)]
pub struct Scroller {
    /// Current position, in lines back from the live screen. Fractional, which
    /// is what lets the view sit between rows.
    pub pos: f32,
    /// Where the position is heading.
    pub target: f32,
    /// Wheel movement accumulated since the last frame.
    ///
    /// Coalescing matters: a trackpad delivers events far faster than the
    /// display refreshes, so applying each one separately would redraw many
    /// times per frame for no visible benefit.
    pub pending: f32,
}

/// How much of the remaining distance is closed per second. Framerate
/// independence comes from raising this to the power of the frame time, so the
/// motion looks the same at 60 Hz and 120 Hz.
const REMAINING_PER_SECOND: f32 = 0.001;

/// Below this, the position is snapped and the animation considered finished.
const EPSILON: f32 = 0.01;

impl Scroller {
    /// Queue movement. Positive scrolls back into history.
    pub fn nudge(&mut self, lines: f32) {
        self.pending += lines;
    }

    /// Jump straight to a position, cancelling any movement in flight.
    pub fn jump_to(&mut self, lines: f32, max: f32) {
        self.target = lines.clamp(0.0, max);
        self.pos = self.target;
        self.pending = 0.0;
    }

    /// Return to the live screen immediately, as typing does.
    pub fn snap_to_bottom(&mut self) {
        self.target = 0.0;
        self.pos = 0.0;
        self.pending = 0.0;
    }

    /// Move both position and target by `lines`, without starting an
    /// animation.
    ///
    /// This is for content sliding *underneath* a scrolled-back view: when
    /// lines retire into scrollback the view has to follow them to stay on the
    /// same text. Nudging would ease toward a new target, which would look
    /// like drift; the view has not moved, the content has.
    pub fn shift(&mut self, lines: f32, max: f32) {
        self.pos = (self.pos + lines).clamp(0.0, max);
        self.target = (self.target + lines).clamp(0.0, max);
    }

    /// Advance by one frame of `dt` seconds. Returns true while still moving.
    pub fn step(&mut self, dt: f32, max: f32) -> bool {
        if self.pending != 0.0 {
            self.target += self.pending;
            self.pending = 0.0;
        }
        self.target = self.target.clamp(0.0, max);
        self.pos = self.pos.clamp(0.0, max);

        let dist = self.target - self.pos;
        if dist.abs() <= EPSILON {
            self.pos = self.target;
            return false;
        }
        let k = 1.0 - REMAINING_PER_SECOND.powf(dt.clamp(0.0, 0.1));
        self.pos += dist * k;
        if (self.target - self.pos).abs() <= EPSILON {
            self.pos = self.target;
            return false;
        }
        true
    }

    /// Whole lines scrolled back, for indexing the grid.
    #[inline]
    pub fn offset(&self) -> usize {
        self.pos.floor().max(0.0) as usize
    }

    /// Sub-line remainder, 0..1 of a cell.
    #[inline]
    pub fn frac(&self) -> f32 {
        self.pos - self.pos.floor()
    }

    /// True when the view is on the live screen and nothing is in flight.
    #[inline]
    pub fn at_bottom(&self) -> bool {
        self.pos <= EPSILON && self.target <= EPSILON && self.pending == 0.0
    }
}
