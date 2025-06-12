use anyhow::Context;
use derive_more::Constructor;
use sqlx::{Pool, Postgres};
use crate::repo::{ensure_only_one_row_updated, ChatIdKind};
use crate::config;
use crate::domain::objects::Announcement;
use crate::domain::primitives::{InternalChatId, LanguageCode, SupportedLanguage};

#[derive(Clone, Constructor)]
pub struct Announcements {
    pool: Pool<Postgres>,
    announcements: config::AnnouncementsConfig,
}

impl Announcements {

    pub async fn get_new(&self, chat_id: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<String>> {
        let maybe_announcement = match self.announcements.get(lang_code) {
            Some(announcement) if self.check_conditions(chat_id, announcement, lang_code).await? => Some((*announcement.text).clone()),
            Some(_) | None => None
        };
        Ok(maybe_announcement)
    }

    async fn check_conditions(&self, chat_id_kind: &ChatIdKind, announcement: &config::Announcement, lang_code: &LanguageCode) -> anyhow::Result<bool> {
        let res = match self.get(chat_id_kind, lang_code).await? {
            _ if self.announcements.max_shows == 0 => false,
            Some(entity) if entity.hash[..] != announcement.hash[..] => {
                self.update(entity.chat_id, lang_code, &announcement.hash).await?;
                true
            }
            Some(entity) if entity.times_shown >= self.announcements.max_shows  =>
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

    async fn get(&self, chat_id_kind: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<Announcement>> {
        sqlx::query_as!(Announcement,
            "SELECT chat_id, hash, times_shown FROM Announcements
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text)
                AND language = $2",
                    chat_id_kind.value() as String, lang_code.to_supported_language() as SupportedLanguage)
            .fetch_optional(&self.pool)
            .await
            .map(|opt| opt.map(Into::into))
            .context(format!("couldn't get the announcement for {chat_id_kind}, {lang_code:?}"))
    }

    async fn create(&self, chat_id_kind: &ChatIdKind, lang_code: &LanguageCode, hash: &[u8]) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO Announcements (chat_id, language, hash, times_shown) VALUES (
                (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text),
                $2, $3, 1)",
                    chat_id_kind.value() as String, lang_code.to_supported_language() as SupportedLanguage, hash)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't create the announcement for {chat_id_kind}, {lang_code:?}, {hash:?}"))
    }

    async fn increment_times_shown(&self, chat_id: InternalChatId, lang_code: &LanguageCode) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET times_shown = times_shown + 1 WHERE chat_id = $1 AND language::text = $2",
                chat_id.0, lang_code.as_str())
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't increment shown times for {chat_id:?}, {lang_code:?}"))
    }

    async fn update(&self, chat_id: InternalChatId, lang_code: &LanguageCode, hash: &[u8]) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET hash = $3, times_shown = 1 WHERE chat_id = $1 AND language = $2",
                chat_id.0, lang_code.to_supported_language() as SupportedLanguage, hash)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't update announcement for {chat_id:?}, {lang_code:?}, {hash:?}"))
    }
}
