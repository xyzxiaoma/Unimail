//! Deterministic retry, clock, random, and cancellation-aware sleep policy.

use std::{future::Future, pin::Pin, time::Duration};

use unimail_core::{Cancellation, ProviderError, ProviderErrorKind, RetryHint};

/// Wall-clock source used for durable retry deadlines.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// Deterministic random source used only to spread retry deadlines.
pub trait RandomSource: Send + Sync {
    fn next_u64(&self) -> u64;
}

/// Result of a cancellation-aware sleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepOutcome {
    Elapsed,
    Cancelled,
}

/// Runtime-neutral future returned by a sleeper adapter.
pub type SleepFuture<'a> = Pin<Box<dyn Future<Output = SleepOutcome> + Send + 'a>>;

/// Cancellation-aware wall-clock sleeper supplied by the composition root.
pub trait Sleeper: Send + Sync {
    fn sleep_until<'a>(
        &'a self,
        deadline_ms: i64,
        cancellation: &'a dyn Cancellation,
    ) -> SleepFuture<'a>;
}

/// Terminal classification for a provider failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryStop {
    NeedsAuth,
    InvalidCursor,
    Failed,
    Cancelled,
}

/// Durable action selected after a provider attempt fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAction {
    WaitUntil(i64),
    Stop(RetryStop),
}

/// Capped exponential backoff with deterministic symmetric jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    base_delay: Duration,
    max_delay: Duration,
    max_attempts: u32,
    jitter_basis_points: u16,
}

impl RetryPolicy {
    /// Creates a retry policy. Jitter is expressed in basis points and may not exceed 100%.
    #[must_use]
    pub const fn new(
        base_delay: Duration,
        max_delay: Duration,
        max_attempts: u32,
        jitter_basis_points: u16,
    ) -> Option<Self> {
        if base_delay.is_zero()
            || max_delay.is_zero()
            || base_delay.as_millis() > max_delay.as_millis()
            || max_attempts == 0
            || jitter_basis_points > 10_000
        {
            None
        } else {
            Some(Self {
                base_delay,
                max_delay,
                max_attempts,
                jitter_basis_points,
            })
        }
    }

    /// Classifies a failed provider attempt and computes its durable wall-clock deadline.
    ///
    /// `failed_attempt` is one-based and includes the attempt that produced `error`.
    #[must_use]
    pub fn action(
        self,
        now_ms: i64,
        failed_attempt: u32,
        error: &ProviderError,
        random: &dyn RandomSource,
    ) -> RetryAction {
        let stop = match error.kind {
            ProviderErrorKind::Authentication | ProviderErrorKind::Permission => {
                Some(RetryStop::NeedsAuth)
            }
            ProviderErrorKind::InvalidCursor => Some(RetryStop::InvalidCursor),
            ProviderErrorKind::Protocol | ProviderErrorKind::Permanent => Some(RetryStop::Failed),
            ProviderErrorKind::Cancelled => Some(RetryStop::Cancelled),
            ProviderErrorKind::Transient | ProviderErrorKind::Throttled => None,
        };
        if let Some(stop) = stop {
            return RetryAction::Stop(stop);
        }
        if failed_attempt >= self.max_attempts {
            return RetryAction::Stop(RetryStop::Failed);
        }

        let delay_ms = match error.retry {
            RetryHint::After(delay) => duration_ms(delay),
            RetryHint::Backoff => self.jittered_backoff_ms(failed_attempt, random),
            RetryHint::Never => return RetryAction::Stop(RetryStop::Failed),
        };
        RetryAction::WaitUntil(now_ms.saturating_add(u64_to_i64(delay_ms)))
    }

    fn jittered_backoff_ms(self, failed_attempt: u32, random: &dyn RandomSource) -> u64 {
        let base_ms = duration_ms(self.base_delay);
        let max_ms = duration_ms(self.max_delay);
        let shift = failed_attempt.saturating_sub(1).min(63);
        let exponential = base_ms.checked_shl(shift).unwrap_or(u64::MAX).min(max_ms);
        let spread = exponential.saturating_mul(u64::from(self.jitter_basis_points)) / 10_000;
        if spread == 0 {
            return exponential;
        }
        let width = spread.saturating_mul(2).saturating_add(1);
        let sample = random.next_u64() % width;
        let jittered = if sample >= spread {
            exponential.saturating_add(sample - spread)
        } else {
            exponential.saturating_sub(spread - sample)
        };
        jittered.min(max_ms)
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use unimail_core::{ProviderError, ProviderErrorKind, RetryHint};

    use super::{RandomSource, RetryAction, RetryPolicy, RetryStop};

    struct FixedRandom(u64);

    impl RandomSource for FixedRandom {
        fn next_u64(&self) -> u64 {
            self.0
        }
    }

    fn policy() -> RetryPolicy {
        RetryPolicy::new(Duration::from_secs(1), Duration::from_secs(8), 4, 2_500)
            .expect("valid retry policy")
    }

    #[test]
    fn retry_after_is_honored_exactly_without_jitter() {
        let error = ProviderError::new(ProviderErrorKind::Throttled, "rate_limited")
            .with_retry(RetryHint::After(Duration::from_millis(2_345)));

        assert_eq!(
            policy().action(10_000, 1, &error, &FixedRandom(u64::MAX)),
            RetryAction::WaitUntil(12_345)
        );
    }

    #[test]
    fn exponential_backoff_is_jittered_and_capped() {
        let error = ProviderError::new(ProviderErrorKind::Transient, "transport_failed")
            .with_retry(RetryHint::Backoff);

        assert_eq!(
            policy().action(0, 1, &error, &FixedRandom(0)),
            RetryAction::WaitUntil(750)
        );
        assert_eq!(
            policy().action(0, 3, &error, &FixedRandom(2_000)),
            RetryAction::WaitUntil(5_000)
        );
    }

    #[test]
    fn attempt_limit_and_non_retryable_kinds_stop() {
        let transient = ProviderError::new(ProviderErrorKind::Transient, "transport_failed")
            .with_retry(RetryHint::Backoff);
        assert_eq!(
            policy().action(0, 4, &transient, &FixedRandom(0)),
            RetryAction::Stop(RetryStop::Failed)
        );
        let no_retry = ProviderError::new(ProviderErrorKind::Transient, "do_not_retry");
        assert_eq!(
            policy().action(0, 1, &no_retry, &FixedRandom(0)),
            RetryAction::Stop(RetryStop::Failed)
        );

        for (kind, stop) in [
            (ProviderErrorKind::Authentication, RetryStop::NeedsAuth),
            (ProviderErrorKind::Permission, RetryStop::NeedsAuth),
            (ProviderErrorKind::Permanent, RetryStop::Failed),
            (ProviderErrorKind::Protocol, RetryStop::Failed),
            (ProviderErrorKind::Cancelled, RetryStop::Cancelled),
            (ProviderErrorKind::InvalidCursor, RetryStop::InvalidCursor),
        ] {
            let error = ProviderError::new(kind, "safe_error");
            assert_eq!(
                policy().action(0, 1, &error, &FixedRandom(0)),
                RetryAction::Stop(stop)
            );
        }
    }
}
