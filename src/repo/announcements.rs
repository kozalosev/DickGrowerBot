use derive_more::Constructor;
use sqlx::{Pool, Postgres};
use crate::repo::{ensure_only_one_row_updated, ChatIdInternal, ChatIdKind};
use crate::config;
use crate::domain::{LanguageCode, SupportedLanguage};

#[derive(sqlx::FromRow)]
struct AnnouncementEntity {
    chat_id: i64,
    hash: Vec<u8>,
    times_shown: isize,
}

struct Announcement {
    chat_id: ChatIdInternal,
    hash: Vec<u8>,
    times_shown: usize,
}

impl From<AnnouncementEntity> for Announcement {
    fn from(value: AnnouncementEntity) -> Self {
        Self {
            chat_id: ChatIdInternal(value.chat_id),
            hash: value.hash,
            times_shown: value.times_shown as usize
        }
    }
}

#[derive(Clone, Debug, Constructor)]
pub struct Announcements {
    pool: Pool<Postgres>,
    announcements: config::AnnouncementsConfig,
}

impl Announcements {

    #[tracing::instrument]
    pub async fn get_new(&self, chat_id: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<String>> {
        let maybe_announcement = match self.announcements.get(lang_code) {
            Some(announcement) if self.check_conditions(chat_id, announcement, lang_code).await? => Some((*announcement.text).clone()),
            Some(_) | None => None
        };
        Ok(maybe_announcement)
    }

    #[tracing::instrument]
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

    #[tracing::instrument]
    async fn get(&self, chat_id_kind: &ChatIdKind, lang_code: &LanguageCode) -> anyhow::Result<Option<Announcement>> {
        sqlx::query_as!(AnnouncementEntity,
            "SELECT chat_id, hash, times_shown FROM Announcements
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text)
                AND language = $2",
                    chat_id_kind.value() as String, lang_code.to_supported_language() as SupportedLanguage)
            .fetch_optional(&self.pool)
            .await
            .map(|opt| opt.map(Into::into))
            .map_err(Into::into)
    }

    #[tracing::instrument]
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
    }

    #[tracing::instrument]
    async fn increment_times_shown(&self, chat_id: ChatIdInternal, lang_code: &LanguageCode) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET times_shown = times_shown + 1 WHERE chat_id = $1 AND language::text = $2",
                chat_id.0, lang_code.as_str())
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
    }

    #[tracing::instrument]
    async fn update(&self, chat_id: ChatIdInternal, lang_code: &LanguageCode, hash: &[u8]) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Announcements SET hash = $3, times_shown = 1 WHERE chat_id = $1 AND language = $2",
                chat_id.0, lang_code.to_supported_language() as SupportedLanguage, hash)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
    }
}
