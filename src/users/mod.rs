#[cfg(test)]
pub mod mock;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use teloxide::types::{UserId, Update};
use tonic::{Code, Response};
use tonic::transport::Channel;
use generated::user_service_client::UserServiceClient as GrpcClient;
use generated::update_user_request::Target;
use generated::{GetUserRequest, UpdateUserRequest, User};
use crate::config::IntegrationsConfig;
use crate::domain::primitives::{LanguageCode, SupportedLanguage};
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality};
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
    inner: GrpcClient<Channel>,
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
            inner: GrpcClient::new(channel),
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
    async fn get_internal_id(&self, uid: UserId) -> Result<i64, tonic::Status> {
        self.get(uid).await?
            .map(|u| u.id)
            .ok_or_else(|| tonic::Status::not_found("user not found"))
    }
}

impl UserServiceClient for UserServiceClientGrpc {
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
}

impl<C: UserServiceClient> LanguageService<C> {
    pub(crate) fn user_service_enabled(&self) -> bool {
        self.users.enabled()
    }

    /// Resolves the effective language for an update: a group's stored language (when set) wins for
    /// everyone and short-circuits the user-service call; otherwise we fall back to the per-user
    /// resolution ([`resolve_language_for`]).
    pub(crate) async fn resolve(&self, update: &Update) -> LanguageCode {
        if let Some(chat) = update.chat()
            && !chat.is_private() && !chat.is_channel()
            && let Some(lang) = self.chat_language(&chat.id.into()).await
        {
            return LanguageCode::new(lang.to_string());
        }
        resolve_language_for(update.from(), &self.users).await
    }

    /// Sets (or clears, with `None`) the chat-wide language and refreshes the local cache so this
    /// instance is immediately consistent.
    pub(crate) async fn set_chat_language(&self, chat_id: &ChatIdPartiality, lang: Option<SupportedLanguage>) -> anyhow::Result<()> {
        self.chats.set_chat_language(chat_id, lang).await?;
        self.chat_cache().insert(chat_id.kind(), CachedLang { lang, at: tokio::time::Instant::now() });
        Ok(())
    }

    /// Fetches a user from the user-service, or `Ok(None)` when the integration is disabled.
    pub(crate) async fn user(&self, uid: UserId) -> Result<Option<User>, tonic::Status> {
        match &self.users {
            UserService::Connected(client) => client.get(uid).await,
            UserService::Disabled => Ok(None),
        }
    }

    /// Updates a user's personal language in the user-service.
    pub(crate) async fn set_user_language(&self, uid: UserId, code: &str) -> Result<(), tonic::Status> {
        match &self.users {
            UserService::Connected(client) => client.set_language(uid, code).await,
            UserService::Disabled => Err(tonic::Status::unavailable("user-service is disabled")),
        }
    }

    fn chat_cache(&self) -> MutexGuard<'_, HashMap<ChatIdKind, CachedLang>> {
        self.chat_cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Read-through TTL cache over [`Chats::get_chat_language`]. Only [`Self::resolve`] uses it.
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
pub async fn init_language_service(config: &IntegrationsConfig, chats: Chats) -> LanguageService<UserServiceClientGrpc> {
    let users = connect_user_service(config).await;
    LanguageService {
        users,
        chats,
        chat_cache: Arc::new(Mutex::new(HashMap::new())),
        chat_ttl: Duration::from_secs(config.chat_language_cache_time_secs),
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
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(cache_time_secs.max(1)));
        interval.tick().await; // consume the immediate first tick
        loop {
            interval.tick().await;
            client.clean_up_cache();
        }
    });
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
    use teloxide::types::{User, UserId};
    use crate::users::generated::{User as ServiceUser, user::Options};
    use crate::users::mock::UserServiceClientMock;
    use super::{resolve_language_for, UserService};

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
