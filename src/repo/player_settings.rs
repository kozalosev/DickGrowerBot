use autometrics::autometrics;
use anyhow::Context;
use crate::domain::primitives::UserId;
use crate::domain::primitives::chat::ChatIdKind;
use crate::repository;

repository!(PlayerSettings,
    /// Flips whether the user is mentioned in the shrink summaries of this chat, starting from what
    /// applies to them now — their own choice for the chat, else the one for every chat, else yes.
    /// Returns the new value, or `None` when the chat or the user is unknown.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, uid = user_id.value()))]
    pub async fn toggle_shrink_mentions(
        &self,
        chat_id: &ChatIdKind,
        user_id: UserId,
    ) -> anyhow::Result<Option<bool>> {
        sqlx::query_scalar!(
            r#"INSERT INTO Player_Settings (chat_id, uid, settings)
                SELECT c.id, u.uid, jsonb_build_object('shrink_mentions', NOT COALESCE(
                    (ps.settings->>'shrink_mentions')::bool, (u.settings->>'shrink_mentions')::bool, true))
                FROM Chats c
                CROSS JOIN Users u
                LEFT JOIN Player_Settings ps ON ps.chat_id = c.id AND ps.uid = u.uid
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text) AND u.uid = $2
                ON CONFLICT (chat_id, uid) DO UPDATE SET settings = Player_Settings.settings || EXCLUDED.settings
                RETURNING (settings->>'shrink_mentions')::bool AS "mentions!""#,
                chat_id.value() as String, user_id as UserId)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't toggle the mentions of the user {user_id} in the chat {chat_id}"))
    }
);
