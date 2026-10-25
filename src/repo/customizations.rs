use std::sync::{Arc, RwLock};
use anyhow::{Context, anyhow, ensure};
use autometrics::autometrics;
use sqlx::{Pool, Postgres};
use crate::config::CustomizationsConfig;
use crate::customization::{Customization, CustomizationName, Decorations};
use crate::domain::primitives::UserId;

#[derive(Clone)]
pub struct Customizations {
    pool: Pool<Postgres>,
    customizations: Arc<RwLock<CustomizationsConfig>>,
}

impl Customizations {
    pub fn new(pool: Pool<Postgres>, customizations: CustomizationsConfig) -> Self {
        Self { pool, customizations: Arc::new(RwLock::new(customizations)) }
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    #[tracing::instrument(skip_all, fields(path = %path))]
    pub fn reload(&self, path: &str) {
        let fresh = CustomizationsConfig::load(path);
        let Ok(mut storage) = self.customizations.write() else {
            tracing::error!("the customizations lock is poisoned, skipping the reload");
            return
        };
        let count = fresh.customizations.len();
        *storage = fresh;
        tracing::info!(customizations = count, "reloaded the customizations");
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), customization_name = %customization_name))]
    pub async fn set_active(&self, uid: UserId, customization_name: CustomizationName) -> anyhow::Result<()> {
        {
            let config = self.customizations.read()
                .map_err(|_| anyhow!("the customizations lock is poisoned"))?;
            ensure!(config.get(&customization_name).is_some(), "unknown customization: {customization_name}");
        }
        sqlx::query!(
            r#"INSERT INTO User_Customizations (uid, customization_name) VALUES ($1, $2)
                   ON CONFLICT (uid) DO UPDATE
                   SET customization_name = EXCLUDED.customization_name, updated_at = current_timestamp"#,
            uid as UserId, &customization_name as &CustomizationName
        )
            .execute(&self.pool)
            .await
            .context(format!("couldn't set customization {customization_name} for {uid}"))
            .map(|_| ())
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn clear_active(&self, uid: UserId) -> anyhow::Result<()> {
        sqlx::query!("DELETE FROM User_Customizations WHERE uid = $1", uid as UserId)
            .execute(&self.pool)
            .await
            .context(format!("couldn't clear the customization of {uid}"))
            .map(|_| ())
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value()))]
    pub async fn get_active(&self, uid: UserId) -> anyhow::Result<Option<CustomizationName>> {
        let stored = sqlx::query_scalar!(
            "SELECT customization_name FROM User_Customizations WHERE uid = $1", uid as UserId)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get the customization of {uid}"))?;
        let config = self.customizations.read()
            .map_err(|_| anyhow!("the customizations lock is poisoned"))?;
        Ok(stored.and_then(|stored| {
            let name = CustomizationName::new(stored);
            resolve_customization(uid, &name, &config).is_some().then_some(name)
        }))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(users = user_ids.len()))]
    pub async fn decorations_for(&self, user_ids: &[UserId]) -> anyhow::Result<Decorations> {
        if user_ids.is_empty() {
            return Ok(Decorations::empty());
        }

        let rows = sqlx::query!(
            r#"SELECT uid AS "uid: UserId", customization_name
                   FROM User_Customizations
                   WHERE uid = ANY($1)"#,
            user_ids as &[UserId],
        )
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't get customizations for {} users", user_ids.len()))?;
        let config = self.customizations.read()
            .map_err(|_| anyhow!("the customizations lock is poisoned"))?;
        Ok(rows.into_iter()
            .filter_map(|row| {
                let name = CustomizationName::new(row.customization_name);
                resolve_customization(row.uid, &name, &config)
                    .map(|customization| (row.uid, customization.clone()))
            })
            .collect())
    }
}

fn resolve_customization<'a>(
    uid: UserId,
    name: &CustomizationName,
    config: &'a CustomizationsConfig,
) -> Option<&'a Customization> {
    let customization = config.get(name);
    if customization.is_none() {
        tracing::warn!(uid = uid.value(), customization_name = %name,
            "an unknown customization name is stored for the user");
    }
    customization
}
