//! Keeping one user, or one chat, from spending the bot's whole allowance of Telegram requests.
//!
//! Two counters per request, each in a fixed window: the sender's and the chat's. The one that has
//! gone further past its limit decides. The counters live in the store, which [`Cache::or_local`]
//! guarantees even where caching is turned off; a store that can't be asked lets the request
//! through, since refusing everybody is worse than letting a flood in.

use std::time::Duration;
use derive_more::Display;
use crate::cache::{Cache, CacheKey};
use crate::config::RateLimitConfig;
use crate::domain::primitives::chat::ChatIdKind;
use crate::domain::primitives::{RateLimit, UserId};

/// What becomes of a request, from the mildest to the harshest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    Allowed,
    /// Over the limit, but still worth telling why nothing else happens.
    Warn,
    /// Over the limit and already told, so dropped without a word.
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

    /// Counts one request against each counter that applies and answers with the harsher verdict.
    pub async fn hit(&self, uid: Option<UserId>, chat: Option<ChatIdKind>) -> Verdict {
        let user = match uid {
            Some(uid) => self.count(UserKey(uid), self.config.user).await,
            None => Verdict::Allowed,
        };
        let chat = match chat {
            Some(chat) => self.count(ChatKey(chat), self.config.chat).await,
            None => Verdict::Allowed,
        };
        user.max(chat)
    }

    async fn count(&self, key: impl CacheKey, limit: RateLimit) -> Verdict {
        if limit.is_zero() {
            return Verdict::Allowed
        }
        let Some(count) = self.cache.increment(key, self.config.window).await else {
            return Verdict::Allowed
        };
        let limit = u64::from(limit.value());
        let warnings = u64::from(self.config.warnings.value());
        if count <= limit {
            Verdict::Allowed
        } else if count <= limit.saturating_add(warnings) {
            Verdict::Warn
        } else {
            Verdict::Ignore
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

#[cfg(test)]
mod test {
    use std::time::Duration;
    use teloxide::types::ChatId;
    use crate::cache::Cache;
    use crate::config::{CacheConfig, CacheMode, RateLimitConfig};
    use crate::domain::primitives::{RateLimit, UserId};
    use super::{RateLimiter, Verdict};

    fn uid(n: u64) -> UserId {
        UserId::new(n)
    }

    async fn limiter(user: u32, chat: u32, warnings: u32) -> RateLimiter {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let config = RateLimitConfig {
            user: RateLimit::new(user),
            chat: RateLimit::new(chat),
            window: Duration::from_mins(1),
            warnings: RateLimit::new(warnings),
        };
        RateLimiter::new(&cache, config)
    }

    #[tokio::test]
    async fn a_user_is_warned_and_then_ignored() {
        let limiter = limiter(2, 0, 3).await;

        let mut verdicts = Vec::new();
        for _ in 0..7 {
            verdicts.push(limiter.hit(Some(uid(1)), None).await);
        }

        use Verdict::*;
        assert_eq!(verdicts, [Allowed, Allowed, Warn, Warn, Warn, Ignore, Ignore]);
    }

    /// Two users sharing a chat each stay within their own limit and still exhaust the chat's.
    #[tokio::test]
    async fn the_chat_limit_catches_what_the_users_do_together() {
        let limiter = limiter(10, 3, 1).await;
        let chat = || Some(ChatId(-100).into());

        let mut verdicts = Vec::new();
        for n in 0..6 {
            verdicts.push(limiter.hit(Some(uid(n % 2)), chat()).await);
        }

        use Verdict::*;
        assert_eq!(verdicts, [Allowed, Allowed, Allowed, Warn, Ignore, Ignore]);
    }

    #[tokio::test]
    async fn a_zero_limit_lets_everything_through() {
        let limiter = limiter(0, 0, 0).await;

        for _ in 0..10 {
            let verdict = limiter.hit(Some(uid(1)), Some(ChatId(-100).into())).await;
            assert_eq!(verdict, Verdict::Allowed);
        }
    }
}
