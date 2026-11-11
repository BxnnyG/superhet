//! Retry schedule of failed deliveries.

use crate::egress::SendError;

/// Seconds to wait before attempt `n + 1` after `n` failed attempts (`n >= 1`): 15 s, 1 min,
/// 4 min, 15 min, 30 min, then hourly.
#[must_use]
pub fn backoff_secs(failed_attempts: u32) -> i32 {
    match failed_attempts {
        0 | 1 => 15,
        2 => 60,
        3 => 240,
        4 => 900,
        5 => 1800,
        _ => 3600,
    }
}

/// What to do after a failed attempt: `Some(delay)` to try again, `None` to give up.
/// `retries` is the number of retries the target allows after the first attempt.
#[must_use]
pub fn next_delay(failed_attempts: u32, retries: u32, error: &SendError) -> Option<i32> {
    (error.retryable && failed_attempts <= retries).then(|| backoff_secs(failed_attempts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_levels_off() {
        let steps: Vec<i32> = (1..=8).map(backoff_secs).collect();
        assert_eq!(steps, [15, 60, 240, 900, 1800, 3600, 3600, 3600]);
    }

    #[test]
    fn transient_errors_retry_until_the_budget_is_used() {
        let transient = SendError::transient("HTTP 503");
        // retries = 2: attempts 1 and 2 fail and are retried, the third failure is final.
        assert_eq!(next_delay(1, 2, &transient), Some(15));
        assert_eq!(next_delay(2, 2, &transient), Some(60));
        assert_eq!(next_delay(3, 2, &transient), None);
        assert_eq!(next_delay(1, 0, &transient), None, "no retries configured");
    }

    #[test]
    fn permanent_errors_are_not_retried() {
        assert_eq!(next_delay(1, 5, &SendError::permanent("HTTP 400")), None);
    }
}
