//! Whether Telegram is refusing this bot's requests often enough that the ones nobody waits for
//! should step aside.
//!
//! Kept in this process: it measures the process's own share of the quota, which is what the
//! refusals are about.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use crate::domain::primitives::RateLimit;
use crate::metrics;

const WINDOW: Duration = Duration::from_mins(1);

#[derive(Clone)]
pub struct TelegramBackoff(Arc<Mutex<State>>);

struct State {
    threshold: RateLimit,
    pause_for: Duration,
    window_started: Instant,
    refusals: u32,
    paused_until: Option<Instant>,
}

impl TelegramBackoff {
    pub fn new(threshold: RateLimit, pause_for: Duration) -> Self {
        Self(Arc::new(Mutex::new(State {
            threshold,
            pause_for,
            window_started: Instant::now(),
            refusals: 0,
            paused_until: None,
        })))
    }

    /// Counts one refusal. Reaching the threshold within a minute starts a pause, or extends the
    /// one already running.
    pub fn record_refusal(&self) {
        let mut state = self.state();
        if state.threshold.is_zero() {
            return
        }
        let now = Instant::now();
        if now.duration_since(state.window_started) >= WINDOW {
            state.window_started = now;
            state.refusals = 0;
        }
        state.refusals = state.refusals.saturating_add(1);
        if state.refusals < state.threshold.value() {
            return
        }

        let was_paused = state.paused_until.is_some();
        state.paused_until = Some(now + state.pause_for);
        if !was_paused {
            metrics::SELF_DESTRUCTION_PAUSED.set(1);
            tracing::warn!(refusals = state.refusals, pause_for = ?state.pause_for,
                "Telegram keeps refusing the requests, pausing the self-destruction of messages");
        }
    }

    pub fn is_backing_off(&self) -> bool {
        let mut state = self.state();
        match state.paused_until {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                state.paused_until = None;
                metrics::SELF_DESTRUCTION_PAUSED.set(0);
                tracing::info!("the load is gone, resuming the self-destruction of messages");
                false
            }
            None => false,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
impl Default for TelegramBackoff {
    fn default() -> Self {
        Self::new(RateLimit::new(0), Duration::ZERO)
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;
    use crate::domain::primitives::RateLimit;
    use super::TelegramBackoff;

    #[tokio::test]
    async fn the_threshold_starts_a_pause_that_runs_out() {
        let backoff = TelegramBackoff::new(RateLimit::new(3), Duration::from_millis(200));

        backoff.record_refusal();
        backoff.record_refusal();
        assert!(!backoff.is_backing_off(), "under the threshold");

        backoff.record_refusal();
        assert!(backoff.is_backing_off(), "at the threshold");

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!backoff.is_backing_off(), "the pause must run out");
    }

    #[tokio::test]
    async fn a_zero_threshold_never_pauses() {
        let backoff = TelegramBackoff::default();

        for _ in 0..100 {
            backoff.record_refusal();
        }
        assert!(!backoff.is_backing_off());
    }
}
