//! Where a half-finished `/promo` or `/support` waits for its next message.
//!
//! This is teloxide's [`Storage`] over our own store, rather than its `RedisStorage`, for four
//! reasons: that one is built on a second Redis client with a connection pool of its own, it keys
//! by the bare chat id, it sets no lifetime at all — so an abandoned dialogue would be kept for
//! ever — and it could not fall back to this process when there is no server to talk to.
//!
//! A dialogue is not a cache: a miss doesn't cost work, it loses a conversation. So the store is
//! asked for one that can't be disabled, and the only thing an operator's choice changes is whether
//! the state survives a restart.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;
use futures::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;
use teloxide::dispatching::dialogue::Storage;
use teloxide::types::ChatId;
use crate::cache::{Cache, CacheKey};

/// Keyed by the chat the dialogue is held in, and by the command it belongs to: a user may be
/// halfway through one command while starting another.
#[derive(derive_more::Display)]
#[display("dialogue:{name}:{}", chat_id.0)]
struct DialogueKey {
    name: &'static str,
    chat_id: ChatId,
}

impl CacheKey for DialogueKey {}

/// The dialogue states of one command, kept apart from every other command's by `name`.
pub struct CachedDialogueStorage<D> {
    cache: Cache,
    name: &'static str,
    ttl: Duration,
    /// `fn() -> D` rather than `D`, so that what the storage holds says nothing about whether the
    /// storage itself may be shared between threads.
    dialogue: PhantomData<fn() -> D>,
}

impl<D> CachedDialogueStorage<D> {
    pub fn new(cache: &Cache, name: &'static str, ttl: Duration) -> Arc<Self> {
        Arc::new(Self { cache: cache.or_local(), name, ttl, dialogue: PhantomData })
    }

    fn key(&self, chat_id: ChatId) -> DialogueKey {
        DialogueKey { name: self.name, chat_id }
    }
}

impl<D> Storage<D> for CachedDialogueStorage<D>
where D: Default + PartialEq + Send + Serialize + DeserializeOwned + 'static
{
    type Error = DialogueStorageError;

    /// Ends the dialogue, and says nothing when there was none: a conversation that never started
    /// is already over, and `Dialogue::exit` is called on both.
    fn remove_dialogue(self: Arc<Self>, chat_id: ChatId) -> BoxFuture<'static, Result<(), Self::Error>> {
        Box::pin(async move {
            self.cache.remove(self.key(chat_id)).await;
            Ok(())
        })
    }

    /// Writes the state and starts its lifetime again, so a dialogue is only forgotten after the
    /// user has stopped answering.
    ///
    /// **The starting state is stored by forgetting it**, because absent and default mean the same
    /// thing to a reader — `Dialogue::get_or_default` turns one into the other. Without that, every
    /// message in every chat writes a key: `enter_dialogue` calls `get_or_default`, which *writes*
    /// the default when it finds nothing, and one of the two branches it sits behind matches every
    /// message there is.
    fn update_dialogue(
        self: Arc<Self>,
        chat_id: ChatId,
        dialogue: D,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        Box::pin(async move {
            if dialogue == D::default() {
                self.cache.remove(self.key(chat_id)).await;
                return Ok(())
            }
            let bytes = serde_json::to_vec(&dialogue)?;
            self.cache.set_bytes(self.key(chat_id), bytes, self.ttl).await;
            Ok(())
        })
    }

    fn get_dialogue(self: Arc<Self>, chat_id: ChatId) -> BoxFuture<'static, Result<Option<D>, Self::Error>> {
        Box::pin(async move {
            self.cache.get_bytes(self.key(chat_id)).await
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()
                .map_err(Into::into)
        })
    }
}

/// The only way any of this fails: serde. Reaching the store never does — it swallows what goes
/// wrong there and answers "nothing known", which for a dialogue reads as one that hasn't started.
#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
#[display("couldn't read or write the dialogue: {_0}")]
pub struct DialogueStorageError(serde_json::Error);

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::{CacheConfig, CacheMode};

    const A_MINUTE: Duration = Duration::from_mins(1);

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    enum TestState {
        #[default]
        Start,
        Requested(String),
    }

    #[tokio::test]
    async fn a_dialogue_survives_a_round_trip() {
        let storage = storage().await;
        let chat_id = ChatId(1);

        let nothing = Arc::clone(&storage).get_dialogue(chat_id).await
            .expect("reading an absent dialogue must succeed");
        assert_eq!(nothing, None);

        Arc::clone(&storage).update_dialogue(chat_id, TestState::Requested("code".to_owned()))
            .await.expect("couldn't write the dialogue");
        let stored = Arc::clone(&storage).get_dialogue(chat_id).await
            .expect("couldn't read the dialogue");
        assert_eq!(stored, Some(TestState::Requested("code".to_owned())));
    }

    /// The regression this guards: `enter_dialogue` calls `Dialogue::get_or_default`, which
    /// *writes* the default when it finds nothing — and one of the two branches it sits behind
    /// matches every message the bot sees. Storing the starting state would be a write per message
    /// in every chat, for a conversation nobody has started.
    #[tokio::test]
    async fn the_starting_state_is_kept_by_keeping_nothing() {
        let storage = storage().await;
        let chat_id = ChatId(2);

        Arc::clone(&storage).update_dialogue(chat_id, TestState::Start)
            .await.expect("couldn't write the dialogue");
        let stored = Arc::clone(&storage).get_dialogue(chat_id).await
            .expect("reading must succeed");
        assert_eq!(stored, None, "the default state must leave no key behind");

        // And it clears a conversation that had started, since absent reads back as the default.
        Arc::clone(&storage).update_dialogue(chat_id, TestState::Requested("code".to_owned()))
            .await.expect("couldn't write the dialogue");
        Arc::clone(&storage).update_dialogue(chat_id, TestState::Start)
            .await.expect("couldn't write the dialogue");
        let stored = Arc::clone(&storage).get_dialogue(chat_id).await
            .expect("reading must succeed");
        assert_eq!(stored, None);
    }

    /// `Dialogue::exit` is called on a conversation that never started — `/promo <code>` answers
    /// straight away — and one that is already over is not an error.
    #[tokio::test]
    async fn ending_a_dialogue_that_is_not_there_is_fine() {
        let storage = storage().await;
        let chat_id = ChatId(7);

        Arc::clone(&storage).remove_dialogue(chat_id).await
            .expect("ending a dialogue nobody started must succeed");

        Arc::clone(&storage).update_dialogue(chat_id, TestState::Requested("code".to_owned()))
            .await.expect("couldn't write the dialogue");
        Arc::clone(&storage).remove_dialogue(chat_id).await
            .expect("couldn't end the dialogue");

        let gone = Arc::clone(&storage).get_dialogue(chat_id).await
            .expect("reading an ended dialogue must succeed");
        assert_eq!(gone, None);
    }

    /// Two chats are two conversations, and so are two commands in one chat.
    #[tokio::test]
    async fn dialogues_of_other_chats_and_commands_are_left_alone() {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let promo: Arc<CachedDialogueStorage<TestState>> = CachedDialogueStorage::new(&cache, "promo", A_MINUTE);
        let support: Arc<CachedDialogueStorage<TestState>> = CachedDialogueStorage::new(&cache, "support", A_MINUTE);

        Arc::clone(&promo).update_dialogue(ChatId(3), TestState::Start)
            .await.expect("couldn't write the dialogue");

        let other_command = Arc::clone(&support).get_dialogue(ChatId(3)).await
            .expect("reading must succeed");
        assert_eq!(other_command, None);
        let other_chat = Arc::clone(&promo).get_dialogue(ChatId(4)).await
            .expect("reading must succeed");
        assert_eq!(other_chat, None);
    }

    /// Nothing here owns a clock but us — the store is local and no container is involved — so the
    /// waiting is advanced through rather than lived through, and the test can use a lifetime of a
    /// realistic length instead of one shortened to keep the suite quick.
    #[tokio::test(start_paused = true)]
    async fn an_abandoned_dialogue_is_forgotten() {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let storage: Arc<CachedDialogueStorage<TestState>> =
            CachedDialogueStorage::new(&cache, "promo", A_MINUTE);

        Arc::clone(&storage).update_dialogue(ChatId(5), TestState::Start)
            .await.expect("couldn't write the dialogue");
        tokio::time::sleep(A_MINUTE + Duration::from_secs(1)).await;

        let gone = Arc::clone(&storage).get_dialogue(ChatId(5)).await
            .expect("reading must succeed");
        assert_eq!(gone, None);
    }

    /// The store an operator switched off keeps dialogues anyway: without one, a command with a
    /// second step could never reach it.
    #[tokio::test]
    async fn a_disabled_store_still_holds_a_dialogue() {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Disabled)).await;
        let storage: Arc<CachedDialogueStorage<TestState>> =
            CachedDialogueStorage::new(&cache, "promo", A_MINUTE);

        let started = TestState::Requested("code".to_owned());
        Arc::clone(&storage).update_dialogue(ChatId(6), started)
            .await.expect("couldn't write the dialogue");
        let stored = Arc::clone(&storage).get_dialogue(ChatId(6)).await
            .expect("couldn't read the dialogue");
        assert_eq!(stored, Some(TestState::Requested("code".to_owned())));
    }

    async fn storage() -> Arc<CachedDialogueStorage<TestState>> {
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        CachedDialogueStorage::new(&cache, "test", A_MINUTE)
    }
}
