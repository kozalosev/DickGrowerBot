//! Keeping one user, or one chat, from spending the bot's whole allowance of Telegram requests.
//!
//! Two counters per request, each in a fixed window: the sender's and the chat's. The one that has
//! gone further past its limit decides. The counters live in the store, which [`Cache::or_local`]
//! guarantees even where caching is turned off; a store that can't be asked lets the request
//! through, since refusing everybody is worse than letting a flood in.

use std::time::Duration;
use derive_more::Display;
use crate::cache::{Cache, CacheKey};
use crate::config::{CommandSpamLimit, RateLimitConfig};
use crate::domain::primitives::chat::ChatIdKind;
use crate::domain::primitives::{RateLimit, UserId};
use crate::metrics::{self, ThresholdCurve};

/// What becomes of a request over the limit, from the mildest to the harshest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Exceeded {
    /// Still worth telling why nothing else happens.
    Warn,
    /// Already told, so dropped without a word.
    Ignore,
}

#[derive(Clone)]
pub struct RateLimiter {
    cache: Cache,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(cache: &Cache, config: RateLimitConfig) -> Self {
        Self { cache: cache.or_local(), config }
    }

    /// How long a window lasts, which is also how long Telegram may keep an answer to a request
    /// that went over it.
    pub fn window(&self) -> Duration {
        self.config.window
    }

    /// Counts one request against each counter that applies and answers with the harsher verdict,
    /// or `None` while both are within their limits.
    pub async fn hit(&self, uid: Option<UserId>, chat: Option<ChatIdKind>) -> Option<Exceeded> {
        let window = self.config.window;
        let (user, chat) = futures::join!(
            self.count(uid.map(UserKey), self.config.user, window, &metrics::RATE_LIMIT_USER_WINDOWS),
            self.count(chat.map(ChatKey), self.config.chat, window, &metrics::RATE_LIMIT_CHAT_WINDOWS),
        );
        user.max(chat)
    }

    /// Counts one promo code attempt against its user.
    pub async fn promo_exceeded(&self, uid: UserId) -> Option<Exceeded> {
        let CommandSpamLimit { requests: limit, window } = self.config.commands.promo;
        self.count(Some(PromoKey(uid)), limit, window, &metrics::PROMO_ATTEMPT_USERS).await
    }

    /// How long a user waits at most once the promo limit is reached.
    pub fn promo_window(&self) -> Duration {
        self.config.commands.promo.window
    }

    /// Counts a bare `/start` against its chat and tells whether it went over the limit.
    pub async fn start_exceeded(&self, chat: ChatIdKind) -> bool {
        let CommandSpamLimit { requests: limit, window } = self.config.commands.start;
        if limit.is_zero() {
            return false
        }
        self.cache.increment(StartKey(chat), window)
            .await
            .is_some_and(|count| count > u64::from(limit.value()))
    }

    async fn count(
        &self,
        key: Option<impl CacheKey>,
        limit: RateLimit,
        window: Duration,
        curve: &ThresholdCurve,
    ) -> Option<Exceeded> {
        let key = key?;
        if limit.is_zero() {
            return None
        }
        let count = self.cache.increment(key, window).await?;
        curve.observe(count);
        let limit = u64::from(limit.value());
        let warnings = u64::from(self.config.warnings.value());
        if count <= limit {
            None
        } else if count <= limit.saturating_add(warnings) {
            Some(Exceeded::Warn)
        } else {
            Some(Exceeded::Ignore)
        }
    }
}

#[derive(Display)]
#[display("ratelimit:user:{_0}")]
struct UserKey(UserId);

impl CacheKey for UserKey {}

#[derive(Display)]
#[display("ratelimit:chat:{}", _0.qualified())]
struct ChatKey(ChatIdKind);

impl CacheKey for ChatKey {}

#[derive(Display)]
#[display("ratelimit:start:{}", _0.qualified())]
struct StartKey(ChatIdKind);

impl CacheKey for StartKey {}

#[derive(Display)]
#[display("ratelimit:promo:user:{_0}")]
struct PromoKey(UserId);

impl CacheKey for PromoKey {}

#[cfg(test)]
mod test {
    use std::time::Duration;
    use futures::{stream, StreamExt};
    use teloxide::types::ChatId;
    use crate::cache::Cache;
    use crate::config::{CacheConfig, CacheMode, CommandSpamLimit, Commands, RateLimitConfig};
    use crate::domain::primitives::{Limit, RateLimit, UserId};
    use once_cell::sync::Lazy;
    use crate::metrics::{self, render_metrics};
    use super::{Exceeded, RateLimiter};

    #[tokio::test]
    async fn a_user_is_warned_and_then_ignored() {
        let limiter = limiter(2, 0, 3).await;

        let verdicts: Vec<_> = stream::iter(0..7)
            .then(|_| limiter.hit(Some(uid(1)), None))
            .collect()
            .await;

        use Exceeded::*;
        assert_eq!(verdicts, [None, None, Some(Warn), Some(Warn), Some(Warn), Some(Ignore), Some(Ignore)]);
    }

    /// Two users sharing a chat each stay within their own limit and still exhaust the chat's.
    #[tokio::test]
    async fn the_chat_limit_catches_what_the_users_do_together() {
        let limiter = limiter(10, 3, 1).await;
        let chat = || Some(ChatId(-100).into());

        let verdicts: Vec<_> = stream::iter(0..6)
            .then(|n| limiter.hit(Some(uid(n % 2)), chat()))
            .collect()
            .await;

        use Exceeded::*;
        assert_eq!(verdicts, [None, None, None, Some(Warn), Some(Ignore), Some(Ignore)]);
    }

    #[tokio::test]
    async fn a_zero_limit_lets_everything_through() {
        let limiter = limiter(0, 0, 0).await;

        for _ in 0..10 {
            let verdict = limiter.hit(Some(uid(1)), Some(ChatId(-100).into())).await;
            assert_eq!(verdict, None);
        }
    }

    /// The counters are shared by every test of the binary and only grow, so what a hit must have
    /// done is to leave at least one in the series.
    #[tokio::test]
    async fn every_counted_request_is_reported_to_the_curves() {
        let limiter = limiter(2, 2, 0).await;

        limiter.hit(Some(uid(1)), Some(ChatId(-100).into())).await;

        assert!(reached("rate_limit_user_windows_total", 1) >= 1);
        assert!(reached("rate_limit_chat_windows_total", 1) >= 1);
    }

    #[tokio::test]
    async fn every_promo_attempt_is_reported_to_its_curve() {
        let limiter = limiter_with_promo(2, 0).await;

        limiter.promo_exceeded(uid(1)).await;

        assert!(reached("promo_attempt_users_total", 1) >= 1);
    }

    /// A limit that is off counts nothing, so nothing may be reported for it. The threshold is one
    /// no other test gets to, since the series are shared.
    #[tokio::test]
    async fn a_limit_that_is_off_reports_nothing() {
        let limiter = limiter(0, 0, 0).await;
        let before = reached("rate_limit_chat_windows_total", 500);

        for _ in 0..501 {
            limiter.hit(None, Some(ChatId(-7).into())).await;
        }

        assert_eq!(reached("rate_limit_chat_windows_total", 500), before);
    }

    #[tokio::test]
    async fn a_chat_gets_one_start_per_window() {
        let limiter = limiter_with_start(1).await;

        let verdicts: Vec<_> = stream::iter(0..3)
            .then(|_| limiter.start_exceeded(ChatId(-100).into()))
            .collect()
            .await;
        let other_chat = limiter.start_exceeded(ChatId(-200).into()).await;

        assert_eq!(verdicts, [false, true, true]);
        assert!(!other_chat);
    }

    #[tokio::test]
    async fn a_zero_start_limit_lets_every_start_through() {
        let limiter = limiter_with_start(0).await;

        for _ in 0..10 {
            let exceeded = limiter.start_exceeded(ChatId(-100).into()).await;
            assert!(!exceeded);
        }
    }

    #[tokio::test]
    async fn a_promo_guesser_is_warned_and_then_ignored() {
        let limiter = limiter_with_promo(2, 1).await;

        let verdicts: Vec<_> = stream::iter(0..4)
            .then(|_| limiter.promo_exceeded(uid(1)))
            .collect()
            .await;
        let other_user = limiter.promo_exceeded(uid(2)).await;

        use Exceeded::*;
        assert_eq!(verdicts, [None, None, Some(Warn), Some(Ignore)]);
        assert_eq!(other_user, None);
    }

    #[tokio::test]
    async fn a_zero_promo_limit_lets_every_attempt_through() {
        let limiter = limiter_with_promo(0, 0).await;

        for _ in 0..10 {
            let verdict = limiter.promo_exceeded(uid(1)).await;
            assert_eq!(verdict, None);
        }
    }

    fn uid(n: u64) -> UserId {
        UserId::new(n)
    }

    /// The curves register on first use, so they are forced here for a test that never observes one.
    fn reached(name: &str, threshold: u64) -> u64 {
        Lazy::force(&metrics::RATE_LIMIT_USER_WINDOWS);
        Lazy::force(&metrics::RATE_LIMIT_CHAT_WINDOWS);
        Lazy::force(&metrics::PROMO_ATTEMPT_USERS);
        let series = format!("{name}{{reached=\"{threshold}\"}} ");
        render_metrics()
            .lines()
            .find_map(|line| line.strip_prefix(&series))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{series} is not exported"))
    }

    async fn limiter(user: u32, chat: u32, warnings: u16) -> RateLimiter {
        limiter_with(user, chat, warnings, 0, 0).await
    }

    async fn limiter_with_start(start: u32) -> RateLimiter {
        limiter_with(0, 0, 0, start, 0).await
    }

    async fn limiter_with_promo(promo: u32, warnings: u16) -> RateLimiter {
        limiter_with(0, 0, warnings, 0, promo).await
    }

    async fn limiter_with(user: u32, chat: u32, warnings: u16, start: u32, promo: u32) -> RateLimiter {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let config = RateLimitConfig {
            user: RateLimit::new(user),
            chat: RateLimit::new(chat),
            window: Duration::from_mins(1),
            warnings: Limit::new(warnings),
            commands: Commands {
                start: CommandSpamLimit {
                    requests: RateLimit::new(start),
                    window: Duration::from_mins(10),
                },
                promo: CommandSpamLimit {
                    requests: RateLimit::new(promo),
                    window: Duration::from_hours(1),
                },
            },
        };
        RateLimiter::new(&cache, config)
    }
}
