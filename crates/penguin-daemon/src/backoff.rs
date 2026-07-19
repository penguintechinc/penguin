//! Exponential backoff for module restarts, ported byte-for-byte from the Go
//! `BackoffConfig.backoffFor` formula in `go-client/internal/daemon/supervisor.go`.
//!
//! The formula caps the un-jittered delay at [`MAX`] and only then multiplies
//! in jitter, exactly matching the Go order of operations. That means a
//! jittered delay can land up to 5% above [`MAX`] — this is the Go behaviour,
//! not a bug, and is preserved here for parity (see [`delay_for`]).
//!
//! Randomness is injectable: [`delay_for`] takes the jitter fraction as a
//! parameter so tests are deterministic, while [`delay_for_random`] is the
//! thin production entry point that draws the fraction from [`rand`].

use std::time::Duration;

use rand::Rng;

/// Starting delay for the first restart attempt (attempt 0), before jitter.
pub const INITIAL: Duration = Duration::from_millis(100);

/// Ceiling on the un-jittered delay. Jitter is applied after this cap, so the
/// final delay [`delay_for`] returns can exceed `MAX` by up to 5%.
pub const MAX: Duration = Duration::from_secs(30);

/// Exponential growth factor applied per restart attempt.
pub const MULTIPLIER: f64 = 2.0;

/// Restart attempts allowed before a module is parked in the failed state.
/// Not used by the formula itself — carried here alongside the other Go
/// `DefaultBackoff`/`Config` defaults so the supervisor has one place to read
/// the whole default set from.
pub const MAX_RESTARTS: u32 = 5;

/// Computes the backoff delay for restart `attempt` (0-indexed).
///
/// `jitter` is the pre-scaled `[0.0, 0.1)` fraction the Go code calls
/// `jitterFrac` (i.e. `rand.Float64() * 0.1`, already applied); `None` skips
/// jitter entirely. The delay is computed as:
///
/// ```text
/// dur = INITIAL * MULTIPLIER^attempt
/// dur = min(dur, MAX)                    // capped BEFORE jitter
/// dur = dur * (1.0 + jitter - 0.05)      // maps jitter in [0,0.1) to [0.95,1.05)
/// ```
///
/// All arithmetic is done in nanoseconds as `f64`, matching Go's
/// `time.Duration(float64(...))` conversions (including the truncate-toward-
/// zero rounding on the final cast) so the two implementations agree bit for
/// bit on every value exercised by the test corpus.
pub fn delay_for(attempt: u32, jitter: Option<f64>) -> Duration {
    let initial_nanos = INITIAL.as_nanos() as f64;
    let max_nanos = MAX.as_nanos() as f64;

    let mut delay_nanos = initial_nanos * MULTIPLIER.powf(f64::from(attempt));
    if delay_nanos > max_nanos {
        delay_nanos = max_nanos;
    }

    if let Some(frac) = jitter {
        delay_nanos *= 1.0 + frac - 0.05;
    }

    Duration::from_nanos(delay_nanos as u64)
}

/// Computes the backoff delay for `attempt` using real randomness for jitter.
///
/// This is what the supervisor calls in production; [`delay_for`] with an
/// explicit fraction is what tests use to get deterministic results.
pub fn delay_for_random(attempt: u32) -> Duration {
    let unit: f64 = rand::rng().random();
    delay_for(attempt, Some(unit * 0.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_go_reference() {
        assert_eq!(INITIAL, Duration::from_millis(100));
        assert_eq!(MAX, Duration::from_secs(30));
        assert_eq!(MULTIPLIER, 2.0);
        assert_eq!(MAX_RESTARTS, 5);
    }

    #[test]
    fn delay_without_jitter_matches_expected_values_for_attempts_0_through_10() {
        // 100ms doubling each attempt, capped at 30s from attempt 9 onward.
        let expected_ms: [u64; 11] = [
            100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
        ];
        for (attempt, want_ms) in (0_u32..).zip(expected_ms) {
            let got = delay_for(attempt, None);
            assert_eq!(got, Duration::from_millis(want_ms), "attempt {attempt}");
        }
    }

    #[test]
    fn jitter_frac_zero_scales_the_delay_by_0_95() {
        // attempt 3 -> 800ms uncapped; frac 0.0 -> multiplier 0.95 -> 760ms.
        let got = delay_for(3, Some(0.0));
        assert_eq!(got, Duration::from_nanos(760_000_000));
    }

    #[test]
    fn jitter_frac_at_the_0_1_edge_scales_the_delay_by_1_05() {
        // Production jitter never reaches exactly 0.1 (see delay_for_random),
        // but delay_for accepts it so the multiplier's top edge is testable
        // directly: multiplier 1.05 -> 840ms.
        let got = delay_for(3, Some(0.1));
        assert_eq!(got, Duration::from_nanos(840_000_000));
    }

    #[test]
    fn jitter_bounds_hold_at_the_max_cap_and_can_exceed_max() {
        // attempt 9 is already capped to 30s before jitter is applied.
        let low = delay_for(9, Some(0.0));
        let high = delay_for(9, Some(0.1));
        assert_eq!(low, Duration::from_nanos(28_500_000_000));
        assert_eq!(high, Duration::from_nanos(31_500_000_000));
        // Proves jitter is applied AFTER the cap: the high end exceeds MAX.
        assert!(high > MAX);
    }

    #[test]
    fn delay_for_random_always_lands_within_the_jitter_bounds() {
        for attempt in [0_u32, 3, 9] {
            let base_nanos = delay_for(attempt, None).as_nanos();
            let low_nanos = base_nanos * 95 / 100;
            let high_nanos = base_nanos * 105 / 100;

            for _ in 0..200 {
                let got_nanos = delay_for_random(attempt).as_nanos();
                assert!(
                    got_nanos >= low_nanos && got_nanos <= high_nanos,
                    "attempt {attempt}: {got_nanos} outside [{low_nanos}, {high_nanos}]"
                );
            }
        }
    }
}
