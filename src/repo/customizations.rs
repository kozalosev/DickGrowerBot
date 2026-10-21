use anyhow::Context;
use autometrics::autometrics;

use crate::customization::{CustomizationName, Decorations};
use crate::domain::primitives::UserId;
use crate::repository;

repository!(Customizations,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), customization_name = %customization_name))]
    pub async fn set_active(&self, uid: UserId, customization_name: CustomizationName) -> anyhow::Result<()> {
        sqlx::query!(
            r#"INSERT INTO User_Customizations (uid, customization_name) VALUES ($1, $2)
                   ON CONFLICT (uid) DO UPDATE
                   SET customization_name = EXCLUDED.customization_name, updated_at = current_timestamp"#,
            uid as UserId, <&str>::from(customization_name)
        )
            .execute(&self.pool)
            .await
            .context(format!("couldn't set customization {customization_name} for {uid}"))
            .map(|_| ())
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn clear_active(&self, uid: UserId) -> anyhow::Result<()> {
        sqlx::query!("DELETE FROM User_Customizations WHERE uid = $1", uid as UserId)
            .execute(&self.pool)
            .await
            .context(format!("couldn't clear the customization of {uid}"))
            .map(|_| ())
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn get_active(&self, uid: UserId) -> anyhow::Result<Option<CustomizationName>> {
        sqlx::query_scalar!("SELECT customization_name FROM User_Customizations WHERE uid = $1", uid as UserId)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get the customization of {uid}"))
            .map(|stored| stored.and_then(|name| parse_stored_name(uid, &name)))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(users = user_ids.len()))]
    pub async fn decorations_for(&self, user_ids: &[UserId]) -> anyhow::Result<Decorations> {
        if user_ids.is_empty() {
            return Ok(Decorations::empty());
        }

        sqlx::query!(
            r#"SELECT uid AS "uid: UserId", customization_name
                   FROM User_Customizations
                   WHERE uid = ANY($1)"#,
            user_ids as &[UserId],
        )
        .fetch_all(&self.pool)
        .await
        .context(format!("couldn't get customizations for {} users", user_ids.len()))
        .map(|rows| rows.into_iter()
            .filter_map(|row| parse_stored_name(row.uid, &row.customization_name).map(|name| (row.uid, name)))
            .collect())
    }
);

fn parse_stored_name(uid: UserId, value: &str) -> Option<CustomizationName> {
    value.parse()
        .inspect_err(|error| {
            tracing::warn!(uid = uid.value(), customization_name = %value, error = %error,
                "an unknown customization name is stored for the user")
        })
        .ok()
}
