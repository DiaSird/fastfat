//! Transfer speed over the last seconds and the time left.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::app::i18n::tf;

/// Span of the samples the transfer speed is computed from.
pub(super) const SPEED_WINDOW: Duration = Duration::from_secs(5);

/// Measures the transfer speed over the last [`SPEED_WINDOW`].
#[derive(Default)]
pub(super) struct SpeedMeter {
    /// When the data transfer started.
    pub(super) started: Option<Instant>,
    /// Recent `(time, bytes done)` samples, oldest first.
    pub(super) samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    /// Records that `done` bytes are transferred now.
    pub(super) fn sample(&mut self, done: u64) {
        let now = Instant::now();
        self.started.get_or_insert(now);
        self.samples.push_back((now, done));
        while self
            .samples
            .front()
            .is_some_and(|&(time, _)| now.duration_since(time) > SPEED_WINDOW)
            && self.samples.len() > 2
        {
            self.samples.pop_front();
        }
    }

    /// Returns the time since the transfer started.
    pub(super) fn elapsed(&self) -> Duration {
        self.started.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// Returns the bytes per second over the recent samples, once at least
    /// a second has been measured.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn bytes_per_second(&self) -> Option<f64> {
        let (first, last) = (self.samples.front()?, self.samples.back()?);
        let seconds = last.0.duration_since(first.0).as_secs_f64();
        (seconds >= 1.0).then(|| last.1.saturating_sub(first.1) as f64 / seconds)
    }
}

/// Formats a duration as hours, minutes and seconds.
pub(super) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        tf("transfer.hms", &[("h", &h), ("m", &m), ("s", &s)])
    } else if m > 0 {
        tf("transfer.ms", &[("m", &m), ("s", &s)])
    } else {
        tf("transfer.s", &[("s", &s)])
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{SpeedMeter, format_duration};

    #[test]
    fn measures_recent_speed() {
        let start = Instant::now();
        let mut meter = SpeedMeter {
            started: Some(start),
            ..SpeedMeter::default()
        };
        assert_eq!(meter.bytes_per_second(), None);
        meter.samples.push_back((start, 0));
        meter
            .samples
            .push_back((start + Duration::from_millis(500), 1_000));
        // Less than a second measured: no estimate yet.
        assert_eq!(meter.bytes_per_second(), None);
        meter
            .samples
            .push_back((start + Duration::from_secs(2), 4_000));
        let speed = meter.bytes_per_second().unwrap_or_default();
        assert!((speed - 2_000.0).abs() < 1.0, "{speed}");
    }

    #[test]
    fn formats_durations() {
        crate::app::i18n::set_language_for_test(crate::app::i18n::Language::English);
        assert_eq!(format_duration(Duration::from_secs(5)), "5s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_duration(Duration::from_secs(3_725)), "1h 2m 5s");
    }
}
