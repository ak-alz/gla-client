use rand::Rng;
use std::time::Duration;

/// Exponential-backoff-with-full-jitter parameters. `jitter_fraction` is the
/// fraction of the (capped) exponential delay that gets randomized in
/// either direction — e.g. `0.2` means the final delay is the capped value
/// ±20%. Full jitter (as opposed to no jitter) is what actually prevents a
/// "retry storm": without it, every agent instance that failed at the same
/// moment (e.g. all of them, right when a shared backend restarts) would
/// retry at exactly the same computed delay and hit the server together
/// again.
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    pub base: Duration,
    pub max: Duration,
    pub jitter_fraction: f64,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        BackoffConfig {
            base: Duration::from_secs(1),
            max: Duration::from_secs(300),
            jitter_fraction: 0.2,
        }
    }
}

/// `attempt` is the number of CONSECUTIVE failures so far (1 = first
/// failure, produces roughly `base`; each further attempt doubles the
/// pre-jitter delay up to `max`).
pub fn compute_backoff(config: &BackoffConfig, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(32); // avoids float overflow at absurd attempt counts
    let exponential = config.base.as_secs_f64() * 2f64.powi(exponent as i32);
    let capped = exponential.min(config.max.as_secs_f64());
    let jitter_span = capped * config.jitter_fraction;
    let jitter = if jitter_span > 0.0 {
        rand::thread_rng().gen_range(-jitter_span..=jitter_span)
    } else {
        0.0
    };
    Duration::from_secs_f64((capped + jitter).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let config = BackoffConfig {
            base: Duration::from_secs(1),
            max: Duration::from_secs(10),
            jitter_fraction: 0.0, // deterministic for this assertion
        };
        assert_eq!(compute_backoff(&config, 1), Duration::from_secs(1));
        assert_eq!(compute_backoff(&config, 2), Duration::from_secs(2));
        assert_eq!(compute_backoff(&config, 3), Duration::from_secs(4));
        assert_eq!(compute_backoff(&config, 4), Duration::from_secs(8));
        assert_eq!(compute_backoff(&config, 5), Duration::from_secs(10)); // capped, not 16
        assert_eq!(compute_backoff(&config, 20), Duration::from_secs(10)); // stays capped
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let config = BackoffConfig {
            base: Duration::from_secs(10),
            max: Duration::from_secs(100),
            jitter_fraction: 0.2,
        };
        for _ in 0..200 {
            let delay = compute_backoff(&config, 3); // capped exponential = 40s
            assert!(
                delay.as_secs_f64() >= 32.0,
                "delay {delay:?} below expected -20% bound"
            );
            assert!(
                delay.as_secs_f64() <= 48.0,
                "delay {delay:?} above expected +20% bound"
            );
        }
    }
}
