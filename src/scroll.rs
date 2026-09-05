//! Scroll position and its easing.
//!
//! Kept separate from the window so the motion can be tested directly.

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
