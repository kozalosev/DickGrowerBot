use std::collections::HashMap;
use anyhow::Context;
use autometrics::autometrics;
use chrono::NaiveDate;
use domain_types::traits::SaturatingInto;
use sqlx::types::JsonValue;
use sqlx::PgConnection;
use crate::domain::primitives::chat::{ChatIdKind, InternalChatId};
use crate::domain::primitives::{PerkId, PerkName, UserId};
use crate::repository;

/// What the perks of one dick know before it changes: each perk's own state, and the day the
/// database is having.
///
/// The date is asked for here rather than taken from the clock of this process, because the
/// once-a-day rule compares `current_date`. A perk that counts days has to count the same ones.
pub struct PerkStatesSnapshot {
    pub today: NaiveDate,
    states: HashMap<PerkId, JsonValue>,
}

impl PerkStatesSnapshot {
    pub fn of(&self, perk_id: PerkId) -> Option<&JsonValue> {
        self.states.get(&perk_id)
    }
}

repository!(PerkStates,
    /// Gives every perk the id its states are keyed by, creating the rows a first run needs.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn register_all(&self, names: &[PerkName]) -> anyhow::Result<HashMap<PerkName, PerkId>> {
        let names: Vec<String> = names.iter()
            .map(|name| name.value().to_owned())
            .collect();
        sqlx::query!(
                "INSERT INTO Perks (name)
                    SELECT unnest($1::text[]) EXCEPT SELECT name FROM Perks
                    ON CONFLICT (name) DO NOTHING",
                &names)
            .execute(&self.pool)
            .await
            .context(format!("couldn't insert the perks new to this run out of {names:?}"))?;
        sqlx::query!(
                r#"SELECT id AS "id: PerkId", name AS "name: PerkName" FROM Perks WHERE name = ANY($1::text[])"#,
                &names)
            .fetch_all(&self.pool)
            .await
            .map(|rows| rows.into_iter()
                .map(|row| (row.name, row.id))
                .collect())
            .context(format!("couldn't read the ids of the perks {names:?}"))
    },

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), chat_id = %chat_id))]
    pub async fn read(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<PerkStatesSnapshot> {
        let row = sqlx::query!(
                r#"SELECT current_date AS "today!", agg.states AS "states?"
                    FROM (
                        SELECT jsonb_object_agg(ps.perk_id::text, ps.state) AS states
                          FROM Perk_States ps
                          JOIN Chats c ON ps.chat_id = c.id
                         WHERE ps.uid = $1 AND (c.chat_id = $2::bigint OR c.chat_instance = $2::text)
                    ) agg"#,
                uid as UserId, chat_id.value() as String)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't read the perk states of {uid} in {chat_id}"))?;
        Ok(PerkStatesSnapshot {
            today: row.today,
            states: row.states.map(parse_states).unwrap_or_default(),
        })
    },

    /// What the perks of a dick know as the transaction sees it.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), internal_chat_id = %chat_id))]
    pub async fn read_internal(
        conn: &mut PgConnection,
        uid: UserId,
        chat_id: InternalChatId,
    ) -> anyhow::Result<PerkStatesSnapshot> {
        let row = sqlx::query!(
                r#"SELECT current_date AS "today!", agg.states AS "states?"
                    FROM (
                        SELECT jsonb_object_agg(ps.perk_id::text, ps.state) AS states
                          FROM Perk_States ps
                         WHERE ps.uid = $1 AND ps.chat_id = $2
                    ) agg"#,
                uid as UserId, chat_id as InternalChatId)
            .fetch_one(conn)
            .await
            .context(format!("couldn't read the perk states of {uid} in the chat with id = {chat_id}"))?;
        Ok(PerkStatesSnapshot {
            today: row.today,
            states: row.states.map(parse_states).unwrap_or_default(),
        })
    },

    /// Stores what a perk remembers. It takes someone else's connection so that the state is
    /// written together with the length it was computed for, or not at all.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id, uid = uid.value(), perk_id = perk_id.value()))]
    pub async fn write_internal(
        conn: &mut PgConnection,
        chat_id: InternalChatId,
        uid: UserId,
        perk_id: PerkId,
        state: &JsonValue,
    ) -> anyhow::Result<()> {
        let perk_id: i16 = perk_id.value().saturating_into();
        sqlx::query!(
                "INSERT INTO Perk_States (chat_id, uid, perk_id, state) VALUES ($1, $2, $3, $4)
                    ON CONFLICT (chat_id, uid, perk_id) DO UPDATE SET state = EXCLUDED.state",
                chat_id as InternalChatId, uid as UserId, perk_id, state)
            .execute(conn)
            .await
            .context(format!("couldn't write the perk state of {uid} in the chat with id = {chat_id}"))?;
        Ok(())
    }
);

/// The states arrive as one jsonb object keyed by the perk id, which is how the whole set costs a
/// single row. A key that isn't a number belongs to no perk, so it is dropped.
fn parse_states(value: JsonValue) -> HashMap<PerkId, JsonValue> {
    let JsonValue::Object(entries) = value else {
        tracing::warn!(value = ?value, "the perk states are not an object");
        return HashMap::default()
    };
    entries.into_iter()
        .filter_map(|(key, state)| key.parse()
            .inspect_err(|e| tracing::warn!(key = %key, error = %e, "a perk state is keyed by something that is not an id"))
            .ok()
            .map(|id: u16| (PerkId::new(id), state)))
        .collect()
}
