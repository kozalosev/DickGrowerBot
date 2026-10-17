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
        let (user, chat) = futures::join!(
            self.count(uid.map(UserKey), self.config.user),
            self.count(chat.map(ChatKey), self.config.chat),
        );
        user.max(chat)
    }

    async fn count(&self, key: Option<impl CacheKey>, limit: RateLimit) -> Option<Exceeded> {
        let key = key?;
        if limit.is_zero() {
            return None
        }
        let count = self.cache.increment(key, self.config.window).await?;
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

#[cfg(test)]
mod test {
    use std::time::Duration;
    use teloxide::types::ChatId;
    use crate::cache::Cache;
    use crate::config::{CacheConfig, CacheMode, RateLimitConfig};
    use crate::domain::primitives::{Limit, RateLimit, UserId};
    use super::{Exceeded, RateLimiter};

    #[tokio::test]
    async fn a_user_is_warned_and_then_ignored() {
        let limiter = limiter(2, 0, 3).await;

        let mut verdicts = Vec::new();
        for _ in 0..7 {
            verdicts.push(limiter.hit(Some(uid(1)), None).await);
        }

        use Exceeded::*;
        assert_eq!(verdicts, [None, None, Some(Warn), Some(Warn), Some(Warn), Some(Ignore), Some(Ignore)]);
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

    fn uid(n: u64) -> UserId {
        UserId::new(n)
    }

    async fn limiter(user: u32, chat: u32, warnings: u16) -> RateLimiter {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let config = RateLimitConfig {
            user: RateLimit::new(user),
            chat: RateLimit::new(chat),
            window: Duration::from_mins(1),
            warnings: Limit::new(warnings),
        };
        RateLimiter::new(&cache, config)
    }
}
