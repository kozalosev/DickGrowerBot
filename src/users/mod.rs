#[cfg(test)]
pub mod mock;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use teloxide::types::{UpdateKind, UserId, Update};
use tonic::{Code, Response};
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::{OtelGrpcLayer, OtelGrpcService};
use tower::Layer;
use generated::user_service_client::UserServiceClient as GrpcClient;
use generated::update_user_request::Target;
use generated::{GetUserRequest, GetUsersRequest, UpdateUserRequest, User};
use crate::config::IntegrationsConfig;
use crate::domain::primitives::{LanguageCode, SupportedLanguage};
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality};
use crate::handlers::utils::try_resolve_chat_id;
use crate::metrics;
use crate::repo::Chats;

pub mod generated {
    tonic::include_proto!("user_service");
}

/// Abstraction over the user-service client so handlers and the language-resolving
/// middleware can be unit-tested against a mock (see [`mock`]).
pub trait UserServiceClient: Clone {
    /// Fetches a user by their Telegram id (`by_external_id = true`).
    /// Returns `Ok(None)` when the user is not registered in the service.
    async fn get(&self, uid: UserId) -> Result<Option<User>, tonic::Status>;

    /// Batch-fetches just the preferred language of many users by their Telegram ids in one
    /// round-trip (`by_external_id = true`). The returned map is keyed by the requested [`UserId`];
    /// ids that aren't registered, or are registered but have no language code, are simply absent
    /// from it. Uncached — see the impl for why.
    async fn get_user_languages(&self, uids: &[UserId]) -> Result<HashMap<UserId, LanguageCode>, tonic::Status>;

    /// Updates the user's preferred language, propagating it to all bots that
    /// read from the same service. Requires the user to be already registered.
    async fn set_language(&self, uid: UserId, code: &str) -> Result<(), tonic::Status>;
}

/// Runtime gate: the integration is either connected to a real service or fully disabled
/// (when `GRPC_ADDR_USER_SERVICE` is unset or the service was unreachable at startup).
#[derive(Clone)]
pub enum UserService<T: UserServiceClient> {
    Connected(T),
    Disabled,
}

impl<T: UserServiceClient> UserService<T> {
    pub fn enabled(&self) -> bool {
        matches!(self, Self::Connected(_))
    }
}

#[derive(Clone)]
struct CachedUser {
    user: Option<User>,
    updated_at: tokio::time::Instant,
}

impl From<Option<User>> for CachedUser {
    fn from(user: Option<User>) -> Self {
        Self { user, updated_at: tokio::time::Instant::now() }
    }
}

/// gRPC implementation with a small TTL cache keyed by the Telegram [`UserId`].
///
/// The cache stores the whole [`User`] (including the internal `user.id`), so once the
/// language-resolving middleware has fetched a user, [`Self::set_language`] can resolve the
/// internal id from the cache without an extra round-trip.
#[derive(Clone)]
pub struct UserServiceClientGrpc {
    inner: GrpcClient<OtelGrpcService<Channel>>,
    cache: Arc<Mutex<HashMap<UserId, CachedUser>>>,
    cache_ttl: Duration,
}

impl UserServiceClientGrpc {
    pub async fn connect(address: String, cache_time_secs: u64, timeout_secs: u64) -> anyhow::Result<Self> {
        let endpoint = if address.contains("://") {
            address
        } else {
            format!("http://{address}")
        };
        let timeout = Duration::from_secs(timeout_secs);
        let channel = Channel::from_shared(endpoint)?
            .timeout(timeout)          // bounds each request, so a hanging service can't stall us
            .connect_timeout(timeout)
            .connect()
            .await?;
        Ok(Self {
            inner: GrpcClient::new(OtelGrpcLayer.layer(channel)),
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_ttl: Duration::from_secs(cache_time_secs),
        })
    }

    /// Locks the cache. It only guards an in-memory cache whose data stays valid regardless, so if
    /// the mutex was ever poisoned (a holder panicked) we recover the guard rather than propagate
    /// the poison — otherwise a single stray panic would turn every later cache access into a panic.
    fn cache(&self) -> MutexGuard<'_, HashMap<UserId, CachedUser>> {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Evicts stale entries; meant to be called periodically from a background task.
    pub fn clean_up_cache(&self) {
        let now = tokio::time::Instant::now();
        let ttl = self.cache_ttl;
        self.cache().retain(|_, cached| now.duration_since(cached.updated_at) <= ttl);
    }

    fn cached_fresh(&self, uid: UserId) -> Option<Option<User>> {
        let now = tokio::time::Instant::now();
        self.cache().get(&uid)
            .filter(|cached| now.duration_since(cached.updated_at) <= self.cache_ttl)
            .map(|cached| cached.user.clone())
    }

    fn cache_put(&self, uid: UserId, user: Option<User>) {
        self.cache().insert(uid, user.into());
    }

    fn cache_evict(&self, uid: UserId) {
        self.cache().remove(&uid);
    }

    /// Resolves the internal service id, reusing the shared cache the middleware populated.
    #[tracing::instrument(skip_all, fields(uid = uid.0))]
    async fn get_internal_id(&self, uid: UserId) -> Result<i64, tonic::Status> {
        self.get(uid).await?
            .map(|u| u.id)
            .ok_or_else(|| tonic::Status::not_found("user not found"))
    }
}

impl UserServiceClient for UserServiceClientGrpc {
    #[tracing::instrument(skip_all, fields(uid = uid.0))]
    async fn get(&self, uid: UserId) -> Result<Option<User>, tonic::Status> {
        if let Some(cached) = self.cached_fresh(uid) {
            metrics::USER_SERVICE.cache_hit();
            return Ok(cached);
        }

        metrics::USER_SERVICE.request_sent();
        let resp = self.inner.clone().get(GetUserRequest {
            id: uid.0 as i64,
            by_external_id: true,
        }).await;
        match resp {
            Ok(resp) => {
                let user = resp.into_inner();
                self.cache_put(uid, Some(user.clone()));
                Ok(Some(user))
            }
            Err(status) if status.code() == Code::NotFound => {
                self.cache_put(uid, None);
                Ok(None)
            }
            Err(status) => Err(status),
        }
    }

    #[tracing::instrument(skip_all, fields(uids = uids.len()))]
    async fn get_user_languages(&self, uids: &[UserId]) -> Result<HashMap<UserId, LanguageCode>, tonic::Status> {
        // Field-masked response, so `User.id` (unset, proto field 1) reads back as 0 — never cache
        // these shells into the shared `get()` cache, or a later `set_language` would resolve the
        // wrong internal id. Nor is the cache worth *reading* here: this is one round-trip whatever
        // the id count, so serving part of the batch from the cache would only shorten the request,
        // not avoid it — every member would have to be cached to skip the wire at all.
        metrics::USER_SERVICE.request_sent();
        let ids = uids.iter().map(|uid| uid.0 as i64).collect();
        let resp = self.inner.clone().get_many(GetUsersRequest {
            ids,
            by_external_id: true,
            fields: Some(::prost_types::FieldMask {
                paths: vec!["options.language_code".to_owned()],
            }),
        }).await?.into_inner();

        let result = uids.iter()
            .filter_map(|&uid| {
                let code = resp.users.get(&(uid.0 as i64))?
                    .options.as_ref()?
                    .language_code.clone()?;
                Some((uid, LanguageCode::new(code)))
            })
            .collect();
        Ok(result)
    }

    #[tracing::instrument(skip_all, fields(uid = uid.0, code = %code))]
    async fn set_language(&self, uid: UserId, code: &str) -> Result<(), tonic::Status> {
        let id = self.get_internal_id(uid).await?;
        self.inner.clone().update(UpdateUserRequest {
            id,
            target: Some(Target::Language(code.to_owned())),
        }).await.map(Response::into_inner)?;
        self.cache_evict(uid);
        Ok(())
    }
}

/// The most frequent supported language among the given language codes, or `None` when none of
/// them is a language we localize. Pure and DB-free so it can be unit-tested directly.
fn most_popular_language<'a>(codes: impl Iterator<Item = &'a LanguageCode>) -> Option<SupportedLanguage> {
    let mut tally: HashMap<SupportedLanguage, usize> = HashMap::new();
    for lang in codes.filter_map(LanguageCode::as_supported_language) {
        *tally.entry(lang).or_default() += 1;
    }
    tally.into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(lang, _)| lang)
}

/// Chat keys to try for an update that carries no `chat` of its own — every inline invocation.
///
/// Such an update names its chat only indirectly: a supergroup's `chat_id` is encoded in the
/// `inline_message_id` (and only a supergroup's — see [`try_resolve_chat_id`]), while a legacy
/// group is known just by the `chat_instance` of a callback query. The id goes first because
/// that's the row the chat-wide language is always written to: `/language` is a group message and
/// its picker callback reads the chat of the message, so both know the real id, and an
/// instance-only row left over from earlier inline use would read back no language at all.
fn inline_chat_candidates(update: &Update, chats_merging: bool) -> Vec<ChatIdKind> {
    let decoded = |inline_message_id: Option<&String>| chats_merging
        .then(|| inline_message_id.map(String::as_str).and_then(try_resolve_chat_id))
        .flatten()
        .map(ChatIdKind::from);

    match &update.kind {
        // A callback query with a message of its own is an in-chat one: `Update::chat()` covered it.
        UpdateKind::CallbackQuery(query) if query.message.is_none() => decoded(query.inline_message_id.as_ref())
            .into_iter()
            .chain([ChatIdKind::from(query.chat_instance.clone())])
            .collect(),
        UpdateKind::ChosenInlineResult(result) => decoded(result.inline_message_id.as_ref())
            .into_iter()
            .collect(),
        // An inline query names no chat whatsoever — it only tells the *kind* of it.
        _ => Vec::new(),
    }
}

#[derive(Clone)]
struct CachedLang {
    lang: Option<SupportedLanguage>,
    at: tokio::time::Instant,
}

/// The single language-resolution service injected into the dispatcher. It owns both sources of
/// truth: the per-user preference in the user-service (gRPC, cross-bot) and the per-chat override
/// stored in our own `Chats` table (with a small TTL cache).
#[derive(Clone)]
pub struct LanguageService<C: UserServiceClient = UserServiceClientGrpc> {
    users: UserService<C>,
    chats: Chats,
    chat_cache: Arc<Mutex<HashMap<ChatIdKind, CachedLang>>>,
    chat_ttl: Duration,
    /// Whether the `chat_id` hidden in an `inline_message_id` may be used to name a chat —
    /// see [`inline_chat_candidates`].
    chats_merging: bool,
}

impl<C: UserServiceClient> LanguageService<C> {
    pub(crate) fn user_service_enabled(&self) -> bool {
        self.users.enabled()
    }

    /// Resolves the effective language for an update: a group's stored language (when set) wins for
    /// everyone and short-circuits the user-service call; otherwise we fall back to the per-user
    /// resolution ([`resolve_language_for`]).
    #[tracing::instrument(skip_all)]
    pub async fn resolve(&self, update: &Update) -> LanguageCode {
        // Anonymously record the sender's Telegram (client) language — our best proxy for the
        // languages the audience actually speaks, independent of any chat/personal override below.
        if let Some(user) = update.from() {
            metrics::USED_LANGUAGE.record(user.language_code.as_deref());
        }
        if let Some(lang) = self.update_chat_language(update).await {
            return LanguageCode::new(lang.to_string());
        }
        resolve_language_for(update.from(), &self.users).await
    }

    /// Wraps this update in a [`LanguageResolver`] instead of resolving its language right away:
    /// the chat-language lookup and the user-service call only happen if some handler dptree
    /// actually routes the update to asks for it.
    pub fn defer(self, update: Update) -> LanguageResolver<C> {
        metrics::UPDATE_KIND.record(&update.kind);
        LanguageResolver { service: self, update }
    }

    /// The chat-wide language of the chat the update happened in, if the chat is known and has one.
    ///
    /// Inline invocations carry no chat at all, so they are looked up by the indirect keys
    /// [`inline_chat_candidates`] digs out of them — the first key that yields a language wins.
    async fn update_chat_language(&self, update: &Update) -> Option<SupportedLanguage> {
        if let Some(chat) = update.chat() {
            if chat.is_private() || chat.is_channel() {
                return None;
            }
            return self.chat_language(&chat.id.into()).await;
        }
        for chat_id in inline_chat_candidates(update, self.chats_merging) {
            if let Some(lang) = self.chat_language(&chat_id).await {
                return Some(lang);
            }
        }
        None
    }

    /// Sets (or clears, with `None`) the chat-wide language and refreshes the local cache so this
    /// instance is immediately consistent.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, lang = ?lang))]
    pub async fn set_chat_language(&self, chat_id: &ChatIdPartiality, lang: Option<SupportedLanguage>) -> anyhow::Result<()> {
        self.chats.set_chat_language(chat_id, lang).await?;
        self.chat_cache().insert(chat_id.kind(), CachedLang { lang, at: tokio::time::Instant::now() });
        Ok(())
    }

    /// Fetches a user from the user-service, or `Ok(None)` when the integration is disabled.
    #[tracing::instrument(skip_all, fields(uid = uid.0))]
    pub async fn user(&self, uid: UserId) -> Result<Option<User>, tonic::Status> {
        match &self.users {
            UserService::Connected(client) => client.get(uid).await,
            UserService::Disabled => Ok(None),
        }
    }

    /// Tallies the most popular supported language among the given users (by their user-service
    /// preference), for choosing the language of a proactive broadcast to a chat with no chat-wide
    /// override. Returns `None` when the service is disabled, none of the users are registered, or
    /// none has a language we localize — the caller then falls back to English.
    #[tracing::instrument(skip_all, fields(uids = uids.len()))]
    pub async fn popular_language(&self, uids: &[UserId]) -> Option<SupportedLanguage> {
        let UserService::Connected(client) = &self.users else { return None };
        let langs = client.get_user_languages(uids).await
            .inspect_err(|status| log::warn!("couldn't batch-fetch user languages for the tally: {status}"))
            .ok()?;
        most_popular_language(langs.values())
    }

    /// Updates a user's personal language in the user-service.
    #[tracing::instrument(skip_all, fields(uid = uid.0, code = %code))]
    pub async fn set_user_language(&self, uid: UserId, code: &str) -> Result<(), tonic::Status> {
        match &self.users {
            UserService::Connected(client) => client.set_language(uid, code).await,
            UserService::Disabled => Err(tonic::Status::unavailable("user-service is disabled")),
        }
    }

    fn chat_cache(&self) -> MutexGuard<'_, HashMap<ChatIdKind, CachedLang>> {
        self.chat_cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Read-through TTL cache over [`Chats::get_chat_language`].
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    async fn chat_language(&self, chat_id: &ChatIdKind) -> Option<SupportedLanguage> {
        let now = tokio::time::Instant::now();
        if let Some(cached) = self.chat_cache().get(chat_id)
            .filter(|cached| now.duration_since(cached.at) <= self.chat_ttl)
        {
            metrics::CHAT_LANGUAGE.cache_hit();
            return cached.lang;
        }

        metrics::CHAT_LANGUAGE.db_query();
        let lang = self.chats.get_chat_language(chat_id).await
            .unwrap_or_else(|e| {
                log::warn!("couldn't fetch the language of {chat_id}: {e:#}");
                None
            });
        self.chat_cache().insert(chat_id.clone(), CachedLang { lang, at: now });
        lang
    }
}

/// Builds the [`LanguageService`], connecting to the user-service when it's configured and spawning
/// a background task to keep the user cache tidy. Falls back to a disabled user-service (Telegram
/// languages only) when it's not configured or unreachable — the chat-language part keeps working.
pub async fn init_language_service(
    config: &IntegrationsConfig,
    chats: Chats,
    chats_merging: bool,
) -> LanguageService<UserServiceClientGrpc> {
    let users = connect_user_service(config).await;
    LanguageService {
        users,
        chats,
        chat_cache: Arc::new(Mutex::new(HashMap::new())),
        chat_ttl: Duration::from_secs(config.chat_language_cache_time_secs),
        chats_merging,
    }
}

async fn connect_user_service(config: &IntegrationsConfig) -> UserService<UserServiceClientGrpc> {
    let Some(cfg) = config.user_service.as_ref() else {
        log::warn!("user-service integration is disabled (GRPC_ADDR_USER_SERVICE is not set)");
        return UserService::Disabled;
    };
    match UserServiceClientGrpc::connect(cfg.address.clone(), cfg.cache_time_secs, cfg.timeout_secs).await {
        Ok(client) => {
            log::info!("connected to user-service at {}", cfg.address);
            spawn_cache_cleanup(client.clone(), cfg.cache_time_secs);
            UserService::Connected(client)
        }
        Err(e) => {
            log::error!("couldn't connect to user-service: {e:#}");
            UserService::Disabled
        }
    }
}

fn spawn_cache_cleanup(client: UserServiceClientGrpc, cache_time_secs: u64) {
    tokio::spawn(metrics::TASK_USER_SERVICE_CACHE_CLEANUP.instrument(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(cache_time_secs.max(1)));
        interval.tick().await; // consume the immediate first tick
        loop {
            interval.tick().await;
            client.clean_up_cache();
        }
    }));
}

/// A `LanguageCode` not yet resolved: constructing this queries nothing, so an update matched
/// by no handler never touches the chat-language lookup or the user-service.
#[derive(Clone)]
pub struct LanguageResolver<C: UserServiceClient = UserServiceClientGrpc> {
    service: LanguageService<C>,
    update: Update,
}

impl<C: UserServiceClient> LanguageResolver<C> {
    pub async fn execute(&self) -> LanguageCode {
        self.service.resolve(&self.update).await
    }
}

/// Resolves the effective language for a single user: the preference stored in user-service (when
/// the service is connected and the user is registered) takes precedence over the Telegram-provided
/// `language_code`; otherwise it falls back to the stateless [`LanguageCode::from_maybe_user`]. The
/// chat-wide override is handled one level up, in [`LanguageService::resolve`].
pub(crate) async fn resolve_language_for<C: UserServiceClient>(
    user: Option<&teloxide::types::User>,
    svc: &UserService<C>,
) -> LanguageCode {
    if let UserService::Connected(client) = svc
        && let Some(user) = user
    {
        match client.get(user.id).await {
            Ok(Some(u)) => {
                if let Some(code) = u.options.and_then(|opts| opts.language_code) {
                    return LanguageCode::new(code);
                }
            }
            Ok(None) => {}
            Err(status) => log::warn!("couldn't fetch the language of {}: {status}", user.id),
        }
    }
    LanguageCode::from_maybe_user(user)
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use sqlx::{Pool, Postgres};
    use teloxide::types::{Chat, ChatId, ChatKind, ChatPublic, ChosenInlineResult, InaccessibleMessage,
        MaybeInaccessibleMessage, MessageId, PublicChatKind, PublicChatSupergroup, Update, UpdateId,
        UpdateKind, User, UserId};
    use crate::domain::primitives::{LanguageCode, SupportedLanguage};
    use crate::config::FeatureToggles;
    use crate::domain::primitives::chat::{ChatIdFull, ChatIdKind, ChatIdSource, TelegramChatId, TelegramChatInstanceId};
    use crate::handlers::utils::callbacks::build_callback_query;
    use crate::handlers::utils::inline_message_id_of;
    use crate::repo::Chats;
    use crate::repo::test::start_postgres;
    use crate::users::generated::{User as ServiceUser, user::Options};
    use crate::users::mock::UserServiceClientMock;
    use super::{inline_chat_candidates, most_popular_language, resolve_language_for, LanguageService, UserService, UserServiceClient};

    fn service_user(id: i64, language_code: Option<&str>) -> ServiceUser {
        ServiceUser {
            id,
            name: None,
            options: Some(Options { language_code: language_code.map(ToOwned::to_owned), location: None }),
            is_premium: false,
        }
    }

    #[test]
    fn most_popular_language_picks_the_mode() {
        let codes = [
            LanguageCode::new("ru".to_owned()),
            LanguageCode::new("ru-RU".to_owned()),
            LanguageCode::new("it".to_owned()),
            LanguageCode::new("xx".to_owned()),    // not a supported language — ignored
        ];
        let lang = most_popular_language(codes.iter());
        assert_eq!(lang, Some(SupportedLanguage::RU));
    }

    #[test]
    fn most_popular_language_none_when_nothing_supported() {
        let codes = [LanguageCode::new("xx".to_owned())];
        assert_eq!(most_popular_language(codes.iter()), None);
        assert_eq!(most_popular_language([].iter()), None);
    }

    #[tokio::test]
    async fn get_user_languages_returns_only_users_with_a_language() {
        let svc = UserServiceClientMock::new();
        let UserService::Connected(client) = &svc else { panic!("mock must be connected") };
        client.insert(UserId(1), service_user(1, Some("ru")));
        client.insert(UserId(2), service_user(2, Some("it")));
        client.insert(UserId(3), service_user(3, None)); // registered, but no language code

        let found = client.get_user_languages(&[UserId(1), UserId(2), UserId(3), UserId(4)]).await
            .expect("get_user_languages must succeed");
        assert_eq!(found.len(), 2);
        assert_eq!(found.get(&UserId(1)).map(ToString::to_string), Some("ru".to_owned()));
        assert_eq!(found.get(&UserId(2)).map(ToString::to_string), Some("it".to_owned()));
        assert!(!found.contains_key(&UserId(3)));
        assert!(!found.contains_key(&UserId(4)));
    }

    fn inline_callback_update(inline_message_id: Option<String>, chat_instance: &str) -> Update {
        let mut query = build_callback_query("whatever".to_owned());
        query.inline_message_id = inline_message_id;
        query.chat_instance = chat_instance.to_owned();
        Update { id: UpdateId(0), kind: UpdateKind::CallbackQuery(query) }
    }

    fn chosen_result_update(inline_message_id: Option<String>) -> Update {
        let result = ChosenInlineResult {
            result_id: "grow".to_owned(),
            from: tg_user(1, None),
            location: None,
            inline_message_id,
            query: String::new(),
        };
        Update { id: UpdateId(0), kind: UpdateKind::ChosenInlineResult(result) }
    }

    #[test]
    fn inline_candidates_try_the_decoded_chat_id_first() {
        let chat_id = -1001100294568i64;
        let update = inline_callback_update(Some(inline_message_id_of(chat_id)), "-987654321");
        let candidates = inline_chat_candidates(&update, true);
        assert_eq!(candidates, vec![
            ChatIdKind::ID(TelegramChatId::new(chat_id)),
            ChatIdKind::Instance(TelegramChatInstanceId::new("-987654321".to_owned())),
        ]);
    }

    #[test]
    fn inline_candidates_fall_back_to_the_chat_instance() {
        let chat_id = -1001100294568i64;
        let instance = ChatIdKind::Instance(TelegramChatInstanceId::new("-987654321".to_owned()));

        // Merging off: the id encoded in the inline_message_id must not be used at all.
        let update = inline_callback_update(Some(inline_message_id_of(chat_id)), "-987654321");
        assert_eq!(inline_chat_candidates(&update, false), vec![instance.clone()]);

        // A legacy group's inline_message_id encodes no chat, and there may be none at all.
        let undecodable = inline_callback_update(Some("not-an-inline-message-id".to_owned()), "-987654321");
        assert_eq!(inline_chat_candidates(&undecodable, true), vec![instance.clone()]);
        let no_id = inline_callback_update(None, "-987654321");
        assert_eq!(inline_chat_candidates(&no_id, true), vec![instance]);
    }

    #[test]
    fn inline_candidates_of_a_chosen_result_and_of_updates_with_a_chat() {
        let chat_id = -1001100294568i64;
        let update = chosen_result_update(Some(inline_message_id_of(chat_id)));
        assert_eq!(inline_chat_candidates(&update, true), vec![ChatIdKind::ID(TelegramChatId::new(chat_id))]);
        // Nothing to fall back to: a chosen result carries no chat_instance.
        assert!(inline_chat_candidates(&chosen_result_update(None), true).is_empty());
        assert!(inline_chat_candidates(&update, false).is_empty());

        // A callback query with a message of its own is resolved by `Update::chat()` instead.
        let mut query = build_callback_query("whatever".to_owned());
        query.chat_instance = "-987654321".to_owned();
        query.message = Some(MaybeInaccessibleMessage::Inaccessible(InaccessibleMessage {
            chat: Chat {
                id: ChatId(chat_id),
                kind: ChatKind::Public(ChatPublic {
                    title: None,
                    kind: PublicChatKind::Supergroup(PublicChatSupergroup {
                        username: None,
                        is_forum: false,
                    }),
                }),
            },
            message_id: MessageId(42),
        }));
        let in_chat = Update { id: UpdateId(0), kind: UpdateKind::CallbackQuery(query) };
        assert!(inline_chat_candidates(&in_chat, true).is_empty());
    }

    fn tg_user(id: u64, language_code: Option<&str>) -> User {
        User {
            id: UserId(id),
            is_bot: false,
            first_name: "tester".to_owned(),
            last_name: None,
            username: None,
            language_code: language_code.map(ToOwned::to_owned),
            is_premium: false,
            added_to_attachment_menu: false,
        }
    }

    /// A supergroup the bot knows by both of its keys — the state an inline invocation has to be
    /// resolved from.
    const SUPERGROUP_ID: i64 = -1001100294568;
    const CHAT_INSTANCE: &str = "-987654321";

    async fn language_service_of(db: Pool<Postgres>, chats_merging: bool) -> LanguageService<UserServiceClientMock> {
        // The repo's own toggle stays on regardless: it's what makes the row keep *both* keys, which
        // is the state to resolve from. The argument is the service's own gate on the decoding.
        let chats = Chats::new(db, FeatureToggles { chats_merging: true, ..Default::default() });
        let full = ChatIdFull {
            id: TelegramChatId::new(SUPERGROUP_ID),
            instance: TelegramChatInstanceId::new(CHAT_INSTANCE.to_owned()),
        };
        chats.set_chat_language(&full.to_partiality(ChatIdSource::Database), Some(SupportedLanguage::RU))
            .await.expect("couldn't set the chat language");

        LanguageService {
            users: UserServiceClientMock::new(),
            chats,
            chat_cache: Arc::new(Mutex::new(HashMap::new())),
            chat_ttl: Duration::from_secs(60),
            chats_merging,
        }
    }

    /// The regression of #138: an inline invocation carries no chat, so the chat-wide language used
    /// to be ignored and everyone got their own. Both keys of the chat must lead to it — and only
    /// a chat we can actually find speaks for its members.
    #[tokio::test]
    async fn chat_language_wins_for_inline_updates() {
        let (_container, db) = start_postgres().await;
        let ls = language_service_of(db.clone(), true).await;
        let en_user = tg_user(1, Some("en"));

        // By the chat id decoded out of the inline_message_id...
        let mut update = inline_callback_update(Some(inline_message_id_of(SUPERGROUP_ID)), CHAT_INSTANCE);
        update.kind = with_sender(update.kind, en_user.clone());
        assert_eq!(ls.resolve(&update).await.to_string(), "ru");

        let chosen = chosen_result_update(Some(inline_message_id_of(SUPERGROUP_ID)));
        assert_eq!(ls.resolve(&chosen).await.to_string(), "ru");

        // ...and, with the decoding off, by the chat_instance alone.
        let ls = language_service_of(db, false).await;
        let mut update = inline_callback_update(None, CHAT_INSTANCE);
        update.kind = with_sender(update.kind, en_user.clone());
        assert_eq!(ls.resolve(&update).await.to_string(), "ru");

        // A chat we know nothing about has no language to impose, so the sender's own wins.
        let mut unknown = inline_callback_update(None, "another-chat-instance");
        unknown.kind = with_sender(unknown.kind, en_user);
        assert_eq!(ls.resolve(&unknown).await.to_string(), "en");
    }

    /// Replaces the sender of a callback query, so the fallback resolves to a known language.
    fn with_sender(kind: UpdateKind, user: User) -> UpdateKind {
        let UpdateKind::CallbackQuery(mut query) = kind else { panic!("a callback query is expected") };
        query.from = user;
        UpdateKind::CallbackQuery(query)
    }

    /// The point of `defer`: constructing a resolver must not touch the database, and only
    /// `execute` actually resolves (and, in doing so, populates the chat-language cache).
    #[tokio::test]
    async fn a_resolver_queries_nothing_until_executed() {
        let (_container, db) = start_postgres().await;
        let ls = language_service_of(db, true).await;
        let update = inline_callback_update(Some(inline_message_id_of(SUPERGROUP_ID)), CHAT_INSTANCE);

        let resolver = ls.clone().defer(update);
        assert!(ls.chat_cache().is_empty());

        assert_eq!(resolver.execute().await.to_string(), "ru");
        assert!(!ls.chat_cache().is_empty());
    }

    #[tokio::test]
    async fn stored_preference_wins_over_telegram() {
        let uid = UserId(1);
        let svc = UserServiceClientMock::new();
        let UserService::Connected(client) = &svc else { panic!("mock must be connected") };
        client.insert(uid, ServiceUser {
            id: 100,
            name: None,
            options: Some(Options { language_code: Some("it".to_owned()), location: None }),
            is_premium: false,
        });

        let user = tg_user(1, Some("ru"));
        let resolved = resolve_language_for(Some(&user), &svc).await;
        assert_eq!(resolved.to_string(), "it");
    }

    #[tokio::test]
    async fn falls_back_to_telegram_then_default() {
        let svc = UserServiceClientMock::new();
        // Unregistered user: fall back to the Telegram-provided code.
        let with_tg = tg_user(2, Some("ru"));
        assert_eq!(resolve_language_for(Some(&with_tg), &svc).await.to_string(), "ru");
        // No Telegram code and no service record: the default.
        let without_tg = tg_user(3, None);
        assert_eq!(resolve_language_for(Some(&without_tg), &svc).await.to_string(), "en");
        // Service disabled entirely: use the Telegram code.
        let disabled = UserService::<UserServiceClientMock>::Disabled;
        assert_eq!(resolve_language_for(Some(&with_tg), &disabled).await.to_string(), "ru");
    }
}
