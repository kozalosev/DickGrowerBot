use autometrics::autometrics;
use std::sync::{Arc, RwLock};
use anyhow::Context;
use sqlx::{Pool, Postgres};
use crate::repo::{ensure_only_one_row_updated, ChatIdKind};
use crate::config;
use crate::domain::objects::Announcement;
use crate::domain::primitives::{Counter, LanguageCode, SupportedLanguage, TextHash};
use crate::domain::primitives::chat::InternalChatId;

#[derive(Clone)]
pub struct Announcements {
    pool: Pool<Postgres>,
    /// Locked so [`Announcements::reload`] can replace it at runtime. Shared by every clone of this
    /// repository, so one reload reaches the whole bot.
    announcements: Arc<RwLock<config::AnnouncementsConfig>>,
}

impl Announcements {
    pub fn new(pool: Pool<Postgres>, announcements: config::AnnouncementsConfig) -> Self {
        Self { pool, announcements: Arc::new(RwLock::new(announcements)) }
    }

    /// Reads `path` again and replaces the announcements. A missing or broken file leaves the bot
    /// without announcements, the same as at startup — [`config::AnnouncementsConfig::load`] logs
    /// the reason and returns an empty config.
    // Called only from the SIGHUP handler, which Windows doesn't have.
    #[cfg_attr(not(unix), allow(dead_code))]
    #[tracing::instrument(skip_all, fields(path = %path))]
    pub fn reload(&self, path: &str) {
        let fresh = config::AnnouncementsConfig::load(path);
        let mut storage = match self.announcements.write() {
            Ok(storage) => storage,
            Err(_) => {
                log::error!("the announcements lock is poisoned, skipping the reload");
                return;
            }
        };
        let count = fresh.announcements.len();
        *storage = fresh;
        log::info!("reloaded the announcements from {path}: {count} languages");
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, lang_code = %lang_code))]
    pub async fn get_new(&self, chat_id: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<String>> {
        let (announcement, max_shows) = {
            let config = match self.announcements.read() {
                Ok(config) => config,
                Err(_) => return Ok(None),
            };
            (config.get(lang_code).cloned(), config.max_shows)
        };
        let maybe_announcement = match announcement {
            Some(ann) if self.check_conditions(chat_id, &ann, max_shows, lang_code).await? => Some(ann.text.to_string()),
            Some(_) | None => None
        };
        Ok(maybe_announcement)
    }

    async fn check_conditions(
        &self,
        chat_id_kind: &ChatIdKind,
        announcement: &config::Announcement,
        max_shows: Counter,
        lang_code: &LanguageCode,
    ) -> anyhow::Result<bool> {
        let res = match self.get(chat_id_kind, lang_code).await? {
            _ if max_shows == Counter::literal(0) => false,
            Some(entity) if entity.hash != *announcement.hash => {
                self.update(entity.chat_id, lang_code, &announcement.hash).await?;
                true
            }
            Some(entity) if entity.times_shown >= max_shows =>
                false,
            Some(entity) => {
                self.increment_times_shown(entity.chat_id, lang_code).await?;
                true
            }
            None => {
                self.create(chat_id_kind, lang_code, &announcement.hash).await?;
                true
            }
        };
        Ok(res)
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id_kind, lang_code = %lang_code))]
    async fn get(&self, chat_id_kind: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<Announcement>> {
        sqlx::query_as!(Announcement,
            r#"SELECT chat_id AS "chat_id: InternalChatId", hash, times_shown AS "times_shown: Counter" FROM Announcements
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text)
                AND language = $2"#,
                    chat_id_kind.value() as String, lang_code.to_supported_language() as SupportedLanguage)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get the announcement for {chat_id_kind}, {lang_code:?}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id_kind, lang_code = %lang_code))]
    async fn create(&self, chat_id_kind: &ChatIdKind, lang_code: &LanguageCode, hash: &TextHash) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO Announcements (chat_id, language, hash, times_shown) VALUES (
                (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text),
                $2, $3, 1)",
                    chat_id_kind.value() as String, lang_code.to_supported_language() as SupportedLanguage, hash as &TextHash)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't create the announcement for {chat_id_kind}, {lang_code:?}, {hash:?}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id, lang_code = %lang_code))]
    async fn increment_times_shown(&self, chat_id: InternalChatId, lang_code: &LanguageCode) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET times_shown = times_shown + 1 WHERE chat_id = $1 AND language::text = $2",
                chat_id as InternalChatId, lang_code as &LanguageCode)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't increment shown times for {chat_id:?}, {lang_code:?}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id, lang_code = %lang_code))]
    async fn update(&self, chat_id: InternalChatId, lang_code: &LanguageCode, hash: &TextHash) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET hash = $3, times_shown = 1 WHERE chat_id = $1 AND language = $2",
                chat_id as InternalChatId, lang_code.to_supported_language() as SupportedLanguage, hash as &TextHash)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't update announcement for {chat_id:?}, {lang_code:?}, {hash:?}"))
    }
}
