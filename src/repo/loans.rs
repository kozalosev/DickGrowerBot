use std::collections::HashMap;
use std::sync::Arc;
use teloxide::types::UserId;
use tokio::sync::RwLock;
use crate::config::FeatureToggles;
use crate::repo;
use crate::repo::{ChatIdKind, ChatIdPartiality};

#[derive(Clone)]
pub struct Loans {
    pool: sqlx::Pool<sqlx::Postgres>,
    features: FeatureToggles,
    chat_id_cache: Arc<RwLock<HashMap<ChatIdKind, i64>>>,
    chats_repo: repo::Chats,
}

impl Loans {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>, features: FeatureToggles) -> Self {
        Self {
            chats_repo: repo::Chats::new(pool.clone(), features),
            pool,
            features,
            chat_id_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_active_loan(&self, uid: UserId, chat_id: ChatIdPartiality) -> anyhow::Result<u32> {
        let internal_chat_id =  match self.get_internal_chat_id(chat_id.kind()).await? {
            Some(id) => id,
            None => return Ok(Default::default())
        };
        sqlx::query_scalar!("SELECT left_to_pay FROM loans WHERE uid = $1 AND chat_id = $2 AND repaid_at IS NULL",
                uid.0 as i64, internal_chat_id)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_loan| maybe_loan.map(|value| value as u32).unwrap_or_default())
            .map_err(|e| e.into())
    }

    pub async fn pay(&self, uid: UserId, chat_id: ChatIdPartiality, value: u32) -> anyhow::Result<()> {
        let internal_chat_id = match self.get_internal_chat_id(chat_id.kind()).await? {
            Some(id) => id,
            None => return Ok(())   // TODO: check or logging?
        };
        sqlx::query!("UPDATE Loans SET left_to_pay = left_to_pay - $3 WHERE uid = $1 AND chat_id = $2 AND repaid_at IS NULL",
                uid.0 as i64, internal_chat_id, value as i64)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| e.into())
    }

    async fn get_internal_chat_id(&self, chat_id: ChatIdKind) -> anyhow::Result<Option<i64>> {
        let maybe_internal_id = self.chat_id_cache
            .read().await
            .get(&chat_id).copied();
        let internal_id = match maybe_internal_id {
            None => {
                let maybe_id = self.chats_repo.get_chat(chat_id.clone())
                    .await?
                    .map(|chat| chat.internal_id);
                if let Some(id) = maybe_id {
                    self.chat_id_cache
                        .write().await
                        .insert(chat_id, id);
                }
                maybe_id
            }
            Some(id) => Some(id)
        };
        Ok(internal_id)
    }
}
