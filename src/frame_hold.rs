//! Holding frames back for synchronized output (DEC mode 2026).
//!
//! A program that repaints in several writes — a status bar, then the text,
//! then the cursor — is visibly torn if the terminal presents a frame partway
//! through. Mode 2026 lets it bracket the whole update: `CSI ? 2026 h` before,
//! `CSI ? 2026 l` after. Between the two the terminal accumulates damage and
//! presents nothing.
//!
//! The clock is passed in rather than read here, so the watchdog can be tested
//! without sleeping.

use std::time::{Duration, Instant};

/// How long a hold may last before it is broken.
///
/// This is not optional. A program that begins an update and then crashes,
/// or is killed, never sends the end sequence — without a watchdog the display
/// would freeze until the terminal was restarted. Everyone implementing this
/// mode uses a timeout in this range.
pub const TIMEOUT: Duration = Duration::from_millis(150);

/// What the caller should do after a poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Present a frame now.
    pub redraw: bool,
    /// The hold overran the watchdog: clear the mode, because the program
    /// that set it is not coming back to clear it itself.
    pub drop_mode: bool,
}

#[derive(Debug, Default)]
pub struct FrameHold {
    /// When the current hold began, if one is in progress.
    since: Option<Instant>,
    /// Damage seen while holding, owed as a redraw when the hold ends.
    pending: bool,
}

impl FrameHold {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while frames are being withheld.
    #[inline]
    pub fn holding(&self) -> bool {
        self.since.is_some()
    }

    /// When the watchdog will fire, so the event loop can wake itself rather
    /// than blocking indefinitely on a hold that nothing will end.
    #[inline]
    pub fn deadline(&self) -> Option<Instant> {
        self.since.map(|t| t + TIMEOUT)
    }

    /// Advance the hold.
    ///
    /// `syncing` is whether the mode is currently set — read *after* parsing
    /// the batch, since a program may begin and end an update inside a single
    /// write, in which case nothing should be held at all. `damaged` is whether
    /// the content changed.
    pub fn poll(&mut self, syncing: bool, damaged: bool, now: Instant) -> Outcome {
        if !syncing {
            // Not holding: release anything owed from a hold that just ended.
            let owed = self.since.take().is_some() && self.pending;
            self.pending = false;
            return Outcome { redraw: damaged || owed, drop_mode: false };
        }

        let started = *self.since.get_or_insert(now);
        self.pending |= damaged;

        if now.duration_since(started) >= TIMEOUT {
            // Broken hold. Give up on it and show what we have.
            self.since = None;
            let owed = self.pending;
            self.pending = false;
            return Outcome { redraw: owed || damaged, drop_mode: true };
        }

        Outcome { redraw: false, drop_mode: false }
    }
}
