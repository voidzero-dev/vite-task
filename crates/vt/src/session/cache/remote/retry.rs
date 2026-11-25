//! Retry policy, jittered exponential backoff, shared `429` cooldown
//! coordination, and per-kind outage circuit breakers for remote cache
//! requests.

use std::{
    fmt::Debug,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use vt_remote_cache::RetryHint;

/// Maximum `Retry-After` delay honored on a `429 Too Many Requests` response.
pub(super) const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

/// Maximum extra jitter added when waking from a shared `429` cooldown so
/// concurrent waiters do not fire in lockstep.
const MAX_COOLDOWN_JITTER: Duration = Duration::from_millis(250);

/// Consecutive exhausted outage failures before the circuit breaker opens.
const OUTAGE_FAILURE_THRESHOLD: u32 = 3;

/// How long the circuit breaker stays open before permitting one probe request.
const OUTAGE_COOLDOWN: Duration = Duration::from_secs(30);

/// Mutually exclusive disposition of an attempt's error for retry, `429`
/// cooldown, and circuit-breaker handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryAction {
    /// Locally cancelled: stop immediately without updating circuit-breaker
    /// success/failure counts (an unfinished probe still re-opens on drop).
    Cancelled,
    /// Protocol-level retry classification from [`RetryHint`].
    Failed(RetryHint),
}

/// Error types that can participate in [`run`].
pub(super) trait RetryableError: Debug {
    /// Classify this error into its mutually exclusive [`RetryAction`].
    fn retry_action(&self) -> RetryAction;
    /// Error returned when the outage circuit breaker is open.
    fn unavailable() -> Self;
    /// Error returned when `cancel_token` cancels a wait between attempts.
    fn cancelled() -> Self;
}

/// Retry parameters for one kind of remote cache operation.
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryPolicy {
    /// Number of retries after the initial attempt (`0` means one attempt total).
    pub max_retries: u32,
    /// Nominal backoff before the first retry; doubles on each subsequent retry.
    pub base_delay: Duration,
    /// Upper bound on computed exponential backoff.
    pub max_delay: Duration,
    /// Upper bound on `Retry-After` delays honored for `429` responses.
    pub max_retry_after: Duration,
    /// Total elapsed time budget for starting retries within one operation.
    /// Does not abort an already in-flight HTTP attempt; prevents slow
    /// timeouts or 30 s server waits from multiplying into multi-minute stalls.
    pub max_elapsed: Duration,
}

impl RetryPolicy {
    /// Retry policy for `POST /fetch` and `GET /blob/{id}` reads: up to 2
    /// retries (3 attempts total) with nominal delays of 250 ms and 500 ms
    /// (jittered), bounded by a 15 s retry-start budget so a dead endpoint
    /// falls back quickly.
    pub const READ: Self = Self {
        max_retries: 2,
        base_delay: Duration::from_millis(250),
        max_delay: Duration::from_secs(5),
        max_retry_after: MAX_RETRY_AFTER,
        max_elapsed: Duration::from_secs(15),
    };
    /// Retry policy for `POST /store` uploads: up to 3 retries (4 attempts
    /// total) with nominal delays of 500 ms, 1 s, and 2 s (jittered), bounded
    /// by a 60 s retry-start budget.
    pub const STORE: Self = Self {
        max_retries: 3,
        base_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        max_retry_after: MAX_RETRY_AFTER,
        max_elapsed: Duration::from_secs(60),
    };

    /// Single-attempt policy with no retries, preserving `max_retry_after` for
    /// recording `429` cooldowns.
    const fn once(self) -> Self {
        Self { max_retries: 0, ..self }
    }

    /// Backoff delay for zero-based `attempt`, with `[0.5x, 1.0x]` equal jitter.
    fn delay(self, attempt: u32) -> Duration {
        let multiplier = 1_u32.checked_shl(attempt).unwrap_or(u32::MAX);
        let nominal = self.base_delay.saturating_mul(multiplier).min(self.max_delay);
        equal_jitter(nominal)
    }
}

/// Retry and circuit-breaker configuration shared across a remote endpoint.
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryConfig {
    pub read: RetryPolicy,
    pub store: RetryPolicy,
    pub outage_threshold: u32,
    pub outage_cooldown: Duration,
}

impl RetryConfig {
    pub const DEFAULT: Self = Self {
        read: RetryPolicy::READ,
        store: RetryPolicy::STORE,
        outage_threshold: OUTAGE_FAILURE_THRESHOLD,
        outage_cooldown: OUTAGE_COOLDOWN,
    };
    /// Zero-delay retry configuration with production attempt counts and
    /// circuit-breaker thresholds, for deterministic tests without backoff
    /// sleeps.
    pub const NO_BACKOFF: Self = Self {
        read: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            max_retry_after: Duration::ZERO,
            ..RetryPolicy::READ
        },
        store: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            max_retry_after: Duration::ZERO,
            ..RetryPolicy::STORE
        },
        ..Self::DEFAULT
    };
}

/// Shared cooldown gate for `429 Too Many Requests` responses.
///
/// Only `429` updates this gate; `5xx` responses (including `503`, which some
/// backends send with a coarse `Retry-After: 60` on transient admission errors)
/// use per-request backoff instead so one `503` does not stall sibling requests.
#[derive(Debug, Default)]
pub(super) struct RateLimit {
    until: Mutex<Option<Instant>>,
}

impl RateLimit {
    /// Wait until any active `429` cooldown has elapsed, with a small random
    /// wake-up spread so concurrent waiters do not resume in lockstep.
    pub async fn wait(&self) {
        let mut waited = false;
        let mut last_remaining = Duration::ZERO;
        loop {
            let until = *self.until.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(until) = until else {
                break;
            };
            let now = Instant::now();
            if until <= now {
                break;
            }
            let remaining = until.saturating_duration_since(now);
            waited = true;
            last_remaining = remaining;
            sleep(remaining).await;
        }
        if waited {
            let spread = (last_remaining / 4).min(MAX_COOLDOWN_JITTER);
            sleep(random_up_to(spread)).await;
        }
    }

    /// Extend the shared cooldown to at least `now + delay`. Concurrent `429`s
    /// never shorten an existing cooldown.
    pub fn record(&self, delay: Duration) {
        if delay.is_zero() {
            return;
        }
        let target = Instant::now() + delay;
        let mut until = self.until.lock().unwrap_or_else(PoisonError::into_inner);
        *until = Some(until.map_or(target, |previous| previous.max(target)));
    }
}

/// Circuit breaker for sustained endpoint outages.
///
/// Opens after `threshold` consecutive exhausted outage failures (network
/// errors or retryable `5xx` responses, excluding `429`), fast-failing later
/// requests for `cooldown`. Once `cooldown` elapses, a single half-open probe
/// request is permitted; if it succeeds, normal requests resume, and if it
/// fails or is cancelled, another cooldown starts.
#[derive(Debug)]
pub(super) struct OutageBreaker {
    threshold: u32,
    cooldown: Duration,
    state: Mutex<OutageState>,
}

#[derive(Debug)]
struct OutageState {
    phase: OutagePhase,
    generation: u64,
}

#[derive(Debug)]
enum OutagePhase {
    Closed { failures: u32 },
    Open { until: Instant },
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutageOutcome {
    Healthy,
    Outage,
}

impl OutageBreaker {
    pub const fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            threshold,
            cooldown,
            state: Mutex::new(OutageState {
                phase: OutagePhase::Closed { failures: 0 },
                generation: 0,
            }),
        }
    }

    /// Try to enter the breaker. Returns `None` while the circuit is open or
    /// another half-open probe is already in flight.
    pub fn enter(&self) -> Option<OutagePermit<'_>> {
        if self.threshold == 0 {
            return Some(OutagePermit {
                breaker: self,
                generation: 0,
                probe: false,
                completed: true,
            });
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let probe = match state.phase {
            OutagePhase::Closed { .. } => false,
            OutagePhase::Open { until } if Instant::now() >= until => {
                state.phase = OutagePhase::HalfOpen;
                true
            }
            OutagePhase::Open { .. } | OutagePhase::HalfOpen => return None,
        };
        let generation = state.generation;
        drop(state);
        Some(OutagePermit { breaker: self, generation, probe, completed: false })
    }
}

/// Active lease on an [`OutageBreaker`]. Dropping an unfinished probe permit
/// (for example on Ctrl-C cancellation) releases the probe slot and starts
/// another cooldown so the circuit cannot get stuck.
pub(super) struct OutagePermit<'a> {
    breaker: &'a OutageBreaker,
    generation: u64,
    probe: bool,
    completed: bool,
}

impl OutagePermit<'_> {
    pub const fn is_probe(&self) -> bool {
        self.probe
    }

    pub fn finish(mut self, outcome: OutageOutcome) {
        if self.completed {
            return;
        }
        let mut state = self.breaker.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.generation == self.generation {
            match (&mut state.phase, outcome) {
                (OutagePhase::Closed { failures }, OutageOutcome::Outage) => {
                    *failures = failures.saturating_add(1);
                    if *failures >= self.breaker.threshold {
                        let count = *failures;
                        state.phase =
                            OutagePhase::Open { until: Instant::now() + self.breaker.cooldown };
                        state.generation = state.generation.wrapping_add(1);
                        tracing::debug!(
                            failures = count,
                            cooldown = ?self.breaker.cooldown,
                            "remote cache circuit opened after repeated failures"
                        );
                    }
                }
                (OutagePhase::Closed { failures }, OutageOutcome::Healthy) => {
                    *failures = 0;
                }
                (OutagePhase::HalfOpen, OutageOutcome::Outage) => {
                    state.phase =
                        OutagePhase::Open { until: Instant::now() + self.breaker.cooldown };
                    state.generation = state.generation.wrapping_add(1);
                    tracing::debug!(
                        cooldown = ?self.breaker.cooldown,
                        "remote cache circuit re-opened after probe failed"
                    );
                }
                (OutagePhase::HalfOpen, OutageOutcome::Healthy) => {
                    state.phase = OutagePhase::Closed { failures: 0 };
                    state.generation = state.generation.wrapping_add(1);
                    tracing::debug!("remote cache circuit closed after probe succeeded");
                }
                (OutagePhase::Open { .. }, _) => {}
            }
        }
        drop(state);
        self.completed = true;
    }
}

impl Drop for OutagePermit<'_> {
    fn drop(&mut self) {
        if self.probe && !self.completed {
            let mut state = self.breaker.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.generation == self.generation && matches!(state.phase, OutagePhase::HalfOpen) {
                state.phase = OutagePhase::Open { until: Instant::now() + self.breaker.cooldown };
                state.generation = state.generation.wrapping_add(1);
            }
        }
    }
}

/// Run `attempt_fn` through `outage`, `rate_limit`, and `policy`, stopping if
/// `cancel_token` is cancelled before an attempt or during a retry wait.
///
/// `attempt_fn` is NOT dropped mid-execution by `cancel_token` here; callers
/// that need cleanup on cancellation (such as joining a blocking file-writer
/// task before deleting a temporary file) honor `cancel_token` inside
/// `attempt_fn` and return an error classified as [`RetryAction::Cancelled`].
pub(super) async fn run<T, E, F, Fut>(
    operation: &'static str,
    policy: RetryPolicy,
    rate_limit: &RateLimit,
    outage: &OutageBreaker,
    cancel_token: &CancellationToken,
    mut attempt_fn: F,
) -> Result<T, E>
where
    E: RetryableError,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    if cancel_token.is_cancelled() {
        return Err(E::cancelled());
    }
    let Some(permit) = outage.enter() else {
        tracing::debug!(operation, "skipping remote cache request while circuit is open");
        return Err(E::unavailable());
    };
    // A half-open probe makes a single attempt so it either recovers or
    // re-opens promptly instead of holding the only probe slot across retries.
    let policy = if permit.is_probe() { policy.once() } else { policy };
    let started = Instant::now();
    let total_attempts = policy.max_retries.saturating_add(1);
    for attempt in 0..total_attempts {
        if cancel_token.run_until_cancelled(rate_limit.wait()).await.is_none() {
            return Err(E::cancelled());
        }
        match attempt_fn().await {
            Ok(value) => {
                permit.finish(OutageOutcome::Healthy);
                return Ok(value);
            }
            Err(error) => match error.retry_action() {
                RetryAction::Cancelled => {
                    // Drop `permit` without finishing so cancelling a probe
                    // re-opens the cooldown and cancelling a normal request
                    // does not reset consecutive failures.
                    return Err(error);
                }
                RetryAction::Failed(RetryHint::Permanent) => {
                    permit.finish(OutageOutcome::Healthy);
                    return Err(error);
                }
                RetryAction::Failed(RetryHint::RateLimited { retry_after }) => {
                    let backoff = policy.delay(attempt);
                    // Floor 429 cooldowns at `backoff` so `Retry-After: 0` cannot
                    // trigger an immediate tight retry loop.
                    let cooldown = retry_after
                        .map_or(backoff, |delay| delay.min(policy.max_retry_after).max(backoff));
                    // Publish the cooldown even on the last attempt so concurrent
                    // requests pause instead of immediately hitting the same 429.
                    rate_limit.record(cooldown);
                    if attempt + 1 >= total_attempts
                        || started.elapsed().saturating_add(cooldown) >= policy.max_elapsed
                    {
                        permit.finish(OutageOutcome::Healthy);
                        return Err(error);
                    }
                    tracing::debug!(
                        operation,
                        attempt = attempt + 1,
                        total_attempts,
                        ?cooldown,
                        ?error,
                        "retrying remote cache request after 429 cooldown"
                    );
                }
                RetryAction::Failed(hint @ (RetryHint::Transient | RetryHint::Outage)) => {
                    let backoff = policy.delay(attempt);
                    if attempt + 1 >= total_attempts
                        || started.elapsed().saturating_add(backoff) >= policy.max_elapsed
                    {
                        let outcome = match hint {
                            RetryHint::Outage => OutageOutcome::Outage,
                            RetryHint::Transient
                            | RetryHint::Permanent
                            | RetryHint::RateLimited { .. } => OutageOutcome::Healthy,
                        };
                        permit.finish(outcome);
                        return Err(error);
                    }
                    tracing::debug!(
                        operation,
                        attempt = attempt + 1,
                        total_attempts,
                        delay = ?backoff,
                        ?error,
                        "retrying remote cache request"
                    );
                    if cancel_token.run_until_cancelled(sleep(backoff)).await.is_none() {
                        return Err(E::cancelled());
                    }
                }
            },
        }
    }
    unreachable!("total_attempts is always at least 1")
}

/// Apply `[0.5x, 1.0x]` equal jitter to `delay` to desynchronize concurrent
/// clients without collapsing backoff to zero.
fn equal_jitter(delay: Duration) -> Duration {
    let Ok(millis) = u64::try_from(delay.as_millis()) else {
        return delay;
    };
    if millis <= 1 {
        return delay;
    }
    let half = millis / 2;
    let span = millis - half;
    let offset = getrandom::u64().map_or(span, |random| random % (span + 1));
    Duration::from_millis(half + offset)
}

/// Random duration uniformly sampled in `[0, max]`.
fn random_up_to(max: Duration) -> Duration {
    let Ok(millis) = u64::try_from(max.as_millis()) else {
        return max;
    };
    if millis == 0 {
        return Duration::ZERO;
    }
    let offset = getrandom::u64().map_or(millis, |random| random % (millis + 1));
    Duration::from_millis(offset)
}

#[expect(
    clippy::disallowed_methods,
    reason = "intentional backoff delay between remote cache retry attempts"
)]
async fn sleep(duration: Duration) {
    if !duration.is_zero() {
        tokio::time::sleep(duration).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum TestError {
        Outage,
        Cancelled,
    }

    impl RetryableError for TestError {
        fn retry_action(&self) -> RetryAction {
            match self {
                Self::Outage => RetryAction::Failed(RetryHint::Outage),
                Self::Cancelled => RetryAction::Cancelled,
            }
        }

        fn unavailable() -> Self {
            Self::Outage
        }

        fn cancelled() -> Self {
            Self::Cancelled
        }
    }

    fn expire_open(breaker: &OutageBreaker) {
        breaker.state.lock().unwrap().phase =
            OutagePhase::Open { until: Instant::now() - Duration::from_secs(1) };
    }

    #[test]
    fn equal_jitter_stays_within_half_to_full_bounds() {
        assert_eq!(equal_jitter(Duration::ZERO), Duration::ZERO);
        assert_eq!(equal_jitter(Duration::from_millis(1)), Duration::from_millis(1));
        for base in [Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(2)]
        {
            for _ in 0..64 {
                let jittered = equal_jitter(base);
                assert!(jittered >= base / 2 && jittered <= base, "{jittered:?} not in half..full");
            }
        }
    }

    #[test]
    fn retry_policy_delay_caps_and_avoids_shift_overflow() {
        let policy = RetryPolicy::STORE;
        for attempt in [0, 1, 2, 10, 64, 100, u32::MAX] {
            let delay = policy.delay(attempt);
            assert!(delay <= policy.max_delay, "attempt {attempt}: {delay:?}");
        }
        let attempt0 = policy.delay(0);
        assert!(attempt0 >= Duration::from_millis(250) && attempt0 <= Duration::from_millis(500));
        let attempt1 = policy.delay(1);
        assert!(attempt1 >= Duration::from_millis(500) && attempt1 <= Duration::from_secs(1));
    }

    #[test]
    fn rate_limit_record_never_shortens_existing_cooldown() {
        let rate_limit = RateLimit::default();
        rate_limit.record(Duration::from_secs(10));
        let first = rate_limit.until.lock().unwrap().unwrap();
        rate_limit.record(Duration::from_secs(1));
        let second = rate_limit.until.lock().unwrap().unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn outage_breaker_opens_probes_recovers_and_handles_dropped_probes() {
        let breaker = OutageBreaker::new(3, Duration::from_secs(30));

        // Non-outage failure (e.g., 429) resets consecutive failure count.
        breaker.enter().unwrap().finish(OutageOutcome::Outage);
        breaker.enter().unwrap().finish(OutageOutcome::Outage);
        breaker.enter().unwrap().finish(OutageOutcome::Healthy);
        assert!(breaker.enter().is_some());

        // Three consecutive outage failures open the circuit.
        for _ in 0..3 {
            breaker.enter().unwrap().finish(OutageOutcome::Outage);
        }
        assert!(breaker.enter().is_none());

        // Expire cooldown: exactly one half-open probe is admitted.
        expire_open(&breaker);
        let dropped_probe = breaker.enter().unwrap();
        assert!(dropped_probe.is_probe());
        assert!(breaker.enter().is_none());
        // Dropping an unfinished probe re-opens the circuit instead of leaking the slot.
        drop(dropped_probe);
        assert!(breaker.enter().is_none());

        // Failed probe re-opens the circuit.
        expire_open(&breaker);
        breaker.enter().unwrap().finish(OutageOutcome::Outage);
        assert!(breaker.enter().is_none());

        // Successful probe closes the circuit.
        expire_open(&breaker);
        breaker.enter().unwrap().finish(OutageOutcome::Healthy);
        assert!(breaker.enter().is_some());
        assert!(breaker.enter().is_some());
    }

    #[tokio::test]
    async fn cancelled_probe_does_not_close_open_circuit() {
        let breaker = OutageBreaker::new(1, Duration::from_secs(30));
        let rate_limit = RateLimit::default();
        let policy =
            RetryPolicy { max_retries: 0, base_delay: Duration::ZERO, ..RetryPolicy::READ };

        let cancel = CancellationToken::new();

        // Open the circuit with one outage failure.
        let err =
            run::<(), TestError, _, _>("test", policy, &rate_limit, &breaker, &cancel, || async {
                Err(TestError::Outage)
            })
            .await
            .unwrap_err();
        assert_eq!(err, TestError::Outage);
        assert!(breaker.enter().is_none());

        // Expire cooldown and cancel the half-open probe: circuit must stay open.
        expire_open(&breaker);
        let err =
            run::<(), TestError, _, _>("test", policy, &rate_limit, &breaker, &cancel, || async {
                Err(TestError::Cancelled)
            })
            .await
            .unwrap_err();
        assert_eq!(err, TestError::Cancelled);
        assert!(breaker.enter().is_none());
    }

    #[tokio::test]
    async fn cancel_stops_retry_backoff_promptly() {
        let breaker = OutageBreaker::new(3, Duration::from_secs(30));
        let rate_limit = RateLimit::default();
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_secs(30),
            max_delay: Duration::from_secs(30),
            max_elapsed: Duration::from_secs(60),
            ..RetryPolicy::READ
        };
        let cancel = CancellationToken::new();
        let cancel_on_attempt = cancel.clone();
        let attempts = std::sync::atomic::AtomicU32::new(0);

        let err =
            run::<(), TestError, _, _>("test", policy, &rate_limit, &breaker, &cancel, || {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                cancel_on_attempt.cancel();
                async { Err(TestError::Outage) }
            })
            .await
            .unwrap_err();
        assert_eq!(err, TestError::Cancelled);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
