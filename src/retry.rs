use std::time::Duration;

use tracing::warn;

use crate::error::Error;

/// Retry policy for transient CLI failures.
///
/// Configure max attempts, backoff strategy, and which errors to retry.
///
/// # Example
///
/// ```
/// use claude_wrapper::RetryPolicy;
/// use std::time::Duration;
///
/// let policy = RetryPolicy::new()
///     .max_attempts(3)
///     .initial_backoff(Duration::from_secs(1))
///     .exponential()
///     .retry_on_timeout(true)
///     .retry_on_exit_codes([1, 2]);
/// ```
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub(crate) max_attempts: u32,
    pub(crate) initial_backoff: Duration,
    pub(crate) max_backoff: Duration,
    pub(crate) backoff_strategy: BackoffStrategy,
    pub(crate) retry_on_timeout: bool,
    pub(crate) retry_exit_codes: Vec<i32>,
}

/// Backoff strategy between retry attempts.
#[derive(Debug, Clone, Copy)]
pub enum BackoffStrategy {
    /// Fixed delay between attempts.
    Fixed,
    /// Exponential backoff (delay doubles each attempt).
    Exponential,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            backoff_strategy: BackoffStrategy::Fixed,
            retry_on_timeout: true,
            retry_exit_codes: Vec::new(),
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default settings (3 attempts, 1s fixed backoff).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of attempts (including the initial attempt).
    ///
    /// A value of 1 means no retries.
    #[must_use]
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }

    /// Set the initial delay before the first retry.
    #[must_use]
    pub fn initial_backoff(mut self, duration: Duration) -> Self {
        self.initial_backoff = duration;
        self
    }

    /// Set the maximum delay between retries (caps exponential growth).
    #[must_use]
    pub fn max_backoff(mut self, duration: Duration) -> Self {
        self.max_backoff = duration;
        self
    }

    /// Use fixed backoff (same delay between each attempt).
    #[must_use]
    pub fn fixed(mut self) -> Self {
        self.backoff_strategy = BackoffStrategy::Fixed;
        self
    }

    /// Use exponential backoff (delay doubles each attempt, capped by max_backoff).
    #[must_use]
    pub fn exponential(mut self) -> Self {
        self.backoff_strategy = BackoffStrategy::Exponential;
        self
    }

    /// Retry on timeout errors.
    #[must_use]
    pub fn retry_on_timeout(mut self, retry: bool) -> Self {
        self.retry_on_timeout = retry;
        self
    }

    /// Retry on specific non-zero exit codes.
    #[must_use]
    pub fn retry_on_exit_codes(mut self, codes: impl IntoIterator<Item = i32>) -> Self {
        self.retry_exit_codes = codes.into_iter().collect();
        self
    }

    /// Calculate the delay for a given attempt (0-indexed).
    pub(crate) fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = match self.backoff_strategy {
            BackoffStrategy::Fixed => self.initial_backoff,
            BackoffStrategy::Exponential => self
                .initial_backoff
                .saturating_mul(2u32.saturating_pow(attempt)),
        };
        delay.min(self.max_backoff)
    }

    /// Check if the given error should be retried.
    pub(crate) fn should_retry(&self, error: &Error) -> bool {
        match error {
            Error::Timeout { .. } => self.retry_on_timeout,
            Error::CommandFailed { exit_code, .. } => self.retry_exit_codes.contains(exit_code),
            _ => false,
        }
    }
}

/// Execute a fallible async operation with retry.
#[cfg(feature = "async")]
pub(crate) async fn with_retry<F, Fut, T>(
    policy: &RetryPolicy,
    mut operation: F,
) -> crate::error::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = crate::error::Result<T>>,
{
    let mut last_error = None;

    for attempt in 0..policy.max_attempts {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt + 1 < policy.max_attempts && policy.should_retry(&e) {
                    let delay = policy.delay_for_attempt(attempt);
                    warn!(
                        attempt = attempt + 1,
                        max_attempts = policy.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retrying after transient error"
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error.expect("at least one attempt was made"))
}

/// Execute a fallible blocking operation with retry. Sync mirror of
/// [`with_retry`]; waits between attempts with [`std::thread::sleep`].
#[cfg(feature = "sync")]
pub(crate) fn with_retry_sync<F, T>(
    policy: &RetryPolicy,
    mut operation: F,
) -> crate::error::Result<T>
where
    F: FnMut() -> crate::error::Result<T>,
{
    let mut last_error = None;

    for attempt in 0..policy.max_attempts {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt + 1 < policy.max_attempts && policy.should_retry(&e) {
                    let delay = policy.delay_for_attempt(attempt);
                    warn!(
                        attempt = attempt + 1,
                        max_attempts = policy.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retrying after transient error"
                    );
                    std::thread::sleep(delay);
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error.expect("at least one attempt was made"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.initial_backoff, Duration::from_secs(1));
        assert!(policy.retry_on_timeout);
        assert!(policy.retry_exit_codes.is_empty());
    }

    #[test]
    fn test_builder() {
        let policy = RetryPolicy::new()
            .max_attempts(5)
            .initial_backoff(Duration::from_millis(500))
            .exponential()
            .retry_on_timeout(false)
            .retry_on_exit_codes([1, 2, 3]);

        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.initial_backoff, Duration::from_millis(500));
        assert!(!policy.retry_on_timeout);
        assert_eq!(policy.retry_exit_codes, vec![1, 2, 3]);
    }

    #[test]
    fn test_fixed_delay() {
        let policy = RetryPolicy::new()
            .initial_backoff(Duration::from_secs(2))
            .fixed();

        assert_eq!(policy.delay_for_attempt(0), Duration::from_secs(2));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(policy.delay_for_attempt(5), Duration::from_secs(2));
    }

    #[test]
    fn test_exponential_delay() {
        let policy = RetryPolicy::new()
            .initial_backoff(Duration::from_secs(1))
            .max_backoff(Duration::from_secs(30))
            .exponential();

        assert_eq!(policy.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(8));
        // Capped at max_backoff
        assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(30));
    }

    #[test]
    fn test_should_retry_timeout() {
        let policy = RetryPolicy::new().retry_on_timeout(true);
        let error = Error::Timeout {
            timeout_seconds: 60,
        };
        assert!(policy.should_retry(&error));

        let policy = RetryPolicy::new().retry_on_timeout(false);
        assert!(!policy.should_retry(&error));
    }

    #[test]
    fn test_should_retry_exit_code() {
        let policy = RetryPolicy::new().retry_on_exit_codes([1, 2]);

        let retryable = Error::CommandFailed {
            command: "test".into(),
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
            working_dir: None,
        };
        assert!(policy.should_retry(&retryable));

        let not_retryable = Error::CommandFailed {
            command: "test".into(),
            exit_code: 99,
            stdout: String::new(),
            stderr: String::new(),
            working_dir: None,
        };
        assert!(!policy.should_retry(&not_retryable));
    }

    #[test]
    fn test_should_not_retry_other_errors() {
        let policy = RetryPolicy::new()
            .retry_on_timeout(true)
            .retry_on_exit_codes([1]);

        let error = Error::NotFound;
        assert!(!policy.should_retry(&error));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_with_retry_succeeds_first_try() {
        let policy = RetryPolicy::new().max_attempts(3);
        let result = with_retry(&policy, || async { Ok::<_, Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_with_retry_succeeds_after_failures() {
        let policy = RetryPolicy::new()
            .max_attempts(3)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(true);

        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();

        let result = with_retry(&policy, || {
            let attempt = attempt_clone.clone();
            async move {
                let n = attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 {
                    Err(Error::Timeout {
                        timeout_seconds: 60,
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_with_retry_exhausts_attempts() {
        let policy = RetryPolicy::new()
            .max_attempts(2)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(true);

        let result: crate::error::Result<()> = with_retry(&policy, || async {
            Err(Error::Timeout {
                timeout_seconds: 60,
            })
        })
        .await;

        assert!(matches!(result, Err(Error::Timeout { .. })));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_with_retry_no_retry_on_non_retryable() {
        let policy = RetryPolicy::new()
            .max_attempts(3)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(false);

        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();

        let result: crate::error::Result<()> = with_retry(&policy, || {
            let attempt = attempt_clone.clone();
            async move {
                attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(Error::Timeout {
                    timeout_seconds: 60,
                })
            }
        })
        .await;

        assert!(result.is_err());
        // Should only attempt once since timeout is not retryable
        assert_eq!(attempt.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn test_with_retry_sync_succeeds_first_try() {
        let policy = RetryPolicy::new().max_attempts(3);
        let result = with_retry_sync(&policy, || Ok::<_, Error>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn test_with_retry_sync_succeeds_after_failures() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = RetryPolicy::new()
            .max_attempts(3)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(true);

        let attempt = AtomicU32::new(0);
        let result = with_retry_sync(&policy, || {
            let n = attempt.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(Error::Timeout {
                    timeout_seconds: 60,
                })
            } else {
                Ok(42)
            }
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(Ordering::SeqCst), 3);
    }

    #[cfg(feature = "sync")]
    #[test]
    fn test_with_retry_sync_exhausts_attempts() {
        let policy = RetryPolicy::new()
            .max_attempts(2)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(true);

        let result: crate::error::Result<()> = with_retry_sync(&policy, || {
            Err(Error::Timeout {
                timeout_seconds: 60,
            })
        });

        assert!(matches!(result, Err(Error::Timeout { .. })));
    }

    #[cfg(feature = "sync")]
    #[test]
    fn test_with_retry_sync_no_retry_on_non_retryable() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = RetryPolicy::new()
            .max_attempts(3)
            .initial_backoff(Duration::from_millis(1))
            .retry_on_timeout(false);

        let attempt = AtomicU32::new(0);
        let result: crate::error::Result<()> = with_retry_sync(&policy, || {
            attempt.fetch_add(1, Ordering::SeqCst);
            Err(Error::Timeout {
                timeout_seconds: 60,
            })
        });

        assert!(result.is_err());
        assert_eq!(attempt.load(Ordering::SeqCst), 1);
    }
}
