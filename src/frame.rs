//! Pace software presentation independently of PTY and input event frequency.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct FramePacer {
    last_present: Option<Instant>,
}

impl FramePacer {
    pub fn deadline(&self, now: Instant, refresh_millihertz: Option<u32>) -> Instant {
        let hz = refresh_millihertz.filter(|&hz| hz > 0).unwrap_or(60_000);
        let interval = Duration::from_secs_f64(1000.0 / hz as f64);
        self.last_present.map_or(now, |last| (last + interval).max(now))
    }

    pub fn presented(&mut self, now: Instant) {
        self.last_present = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_share_a_deadline_and_idle_input_is_immediate() {
        let mut pacer = FramePacer::default();
        let now = Instant::now();
        assert_eq!(pacer.deadline(now, None), now);
        pacer.presented(now);
        let due = pacer.deadline(now, None);
        for ms in 1..16 {
            assert_eq!(pacer.deadline(now + Duration::from_millis(ms), None), due);
        }
        let later = now + Duration::from_secs(1);
        assert_eq!(pacer.deadline(later, None), later);
    }

    #[test]
    fn follows_display_refresh_and_handles_unknown_rates() {
        let mut pacer = FramePacer::default();
        let now = Instant::now();
        pacer.presented(now);
        let at_60 = pacer.deadline(now, Some(60_000)) - now;
        let at_120 = pacer.deadline(now, Some(120_000)) - now;
        assert!((at_60.as_secs_f64() - at_120.as_secs_f64() * 2.0).abs() < 1e-8);
        assert_eq!(pacer.deadline(now, Some(0)), pacer.deadline(now, None));
    }
}
