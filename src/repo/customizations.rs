use anyhow::Context;
use autometrics::autometrics;

use crate::customization::{CustomizationId, Decorations};
use crate::domain::primitives::UserId;
use crate::repository;

repository!(
    Customizations,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), customization_id = %customization_id))]
    pub async fn set_active(
        &self,
        uid: UserId,
        customization_id: CustomizationId,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO User_Customizations (uid, customization_id) VALUES ($1, $2) \
             ON CONFLICT (uid) DO UPDATE \
             SET customization_id = EXCLUDED.customization_id, updated_at = current_timestamp",
            uid as UserId,
            customization_id.as_str(),
        )
        .execute(&self.pool)
        .await
        .context(format!(
            "couldn't set customization {customization_id} for {uid}"
        ))?;
        Ok(())
    },
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn clear_active(&self, uid: UserId) -> anyhow::Result<()> {
        sqlx::query!(
            "DELETE FROM User_Customizations WHERE uid = $1",
            uid as UserId
        )
        .execute(&self.pool)
        .await
        .context(format!("couldn't clear the customization of {uid}"))?;
        Ok(())
    },
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn get_active(&self, uid: UserId) -> anyhow::Result<Option<CustomizationId>> {
        let stored = sqlx::query_scalar!(
            "SELECT customization_id FROM User_Customizations WHERE uid = $1",
            uid as UserId,
        )
        .fetch_optional(&self.pool)
        .await
        .context(format!("couldn't get the customization of {uid}"))?;
        Ok(stored.and_then(|id| parse_stored_id(uid, &id)))
    },
    #[autometrics]
    #[tracing::instrument(skip_all, fields(users = user_ids.len()))]
    pub async fn decorations_for(&self, user_ids: &[UserId]) -> anyhow::Result<Decorations> {
        if user_ids.is_empty() {
            return Ok(Decorations::empty());
        }

        let rows = sqlx::query!(
            r#"SELECT uid AS "uid: UserId", customization_id
                 FROM User_Customizations
                WHERE uid = ANY($1)"#,
            user_ids as &[UserId],
        )
        .fetch_all(&self.pool)
        .await
        .context(format!(
            "couldn't get customizations for {} users",
            user_ids.len()
        ))?;

        Ok(Decorations::from_entries(rows.into_iter().filter_map(
            |row| parse_stored_id(row.uid, &row.customization_id).map(|id| (row.uid, id)),
        )))
    }
);

fn parse_stored_id(uid: UserId, value: &str) -> Option<CustomizationId> {
    value
        .parse()
        .inspect_err(|error| {
            tracing::warn!(
                uid = uid.value(),
                customization_id = %value,
                error = %error,
                "an unknown customization id is stored for the user"
            )
        })
        .ok()
}
