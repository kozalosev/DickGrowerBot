use anyhow::Context;
use crate::domain::primitives::{DaysCount, Length, Ratio, UserId, Username};
use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId};
use crate::repository;

/// A single shrink applied during the daily job. Carries the post-shrink length (from the
/// `UPDATE ... RETURNING`) and the nullable messageable Telegram chat id used to address the
/// broadcast — `None` for inline-only chats the bot can't message proactively.
pub struct ShrinkEvent {
    pub uid: UserId,
    pub owner_name: Username,
    pub lost_length: Length,
    pub new_length: Length,
    pub messageable_chat_id: Option<TelegramChatId>,
}

/// A historical shrink row for the inline `shrinks` command. The `Stale_Dick_Shrinks` log only
/// stores how much was lost, so there's no post-shrink length here (unlike [`ShrinkEvent`]).
pub struct RecentShrink {
    pub owner_name: Username,
    pub lost_length: Length,
}

repository!(Shrinks,
    /// Shrinks every dick that is still positive and hasn't been grown for `grace_days`, logging
    /// each shrink into `Stale_Dick_Shrinks` and returning the events for the broadcast. The whole thing
    /// is one statement: Postgres runs the unreferenced `logged` data-modifying CTE to completion.
    ///
    /// The full `ratio` doesn't apply from day one of staleness: it ramps up linearly over
    /// `ramp_up_days`, starting at `ratio / ramp_up_days` on the first overdue day and reaching
    /// the full `ratio` once a dick has been overdue for `ramp_up_days` days (and staying there
    /// afterwards) — so neglect is punished gradually rather than with one abrupt cut the moment
    /// the grace period lapses. `ramp_up_days <= 1` reproduces the old instant-full-ratio behavior.
    pub async fn perform_daily_shrink(
        &self,
        ratio: Ratio,
        grace_days: DaysCount,
        ramp_up_days: DaysCount,
    ) -> anyhow::Result<Vec<ShrinkEvent>> {
        let grace_days = grace_days.value() as i32;
        let ramp_up_days = ramp_up_days.value() as i32;
        sqlx::query_as!(ShrinkEvent,
            r#"WITH victims AS (
                    SELECT d.uid, d.chat_id,
                           LEAST(d.length, GREATEST(1, CEIL(d.length * $1::double precision * LEAST(1.0,
                               (EXTRACT(DAY FROM (current_timestamp - d.updated_at))::int - $2::int + 1)::double precision
                                   / GREATEST($3::int, 1)
                           ))::bigint)) AS loss
                    FROM Dicks d
                    WHERE d.length > 0
                      AND d.updated_at <= current_timestamp - make_interval(days => $2::int)
                ),
                updated AS (
                    UPDATE Dicks d SET length = d.length - v.loss, bonus_attempts = d.bonus_attempts + 1
                    FROM victims v WHERE d.uid = v.uid AND d.chat_id = v.chat_id
                    RETURNING d.uid, d.chat_id, v.loss AS loss, d.length AS new_length
                ),
                logged AS (
                    INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length)
                    SELECT chat_id, uid, loss FROM updated
                )
                SELECT u.uid AS "uid: UserId", usr.name AS "owner_name: Username",
                       u.loss AS "lost_length!: Length", u.new_length AS "new_length!: Length",
                       c.chat_id AS "messageable_chat_id: TelegramChatId"
                FROM updated u
                JOIN Users usr USING (uid)
                JOIN Chats c ON c.id = u.chat_id"#,
                ratio as Ratio, grace_days, ramp_up_days)
            .fetch_all(&self.pool)
            .await
            .context("couldn't perform the daily shrink")
    },

    pub async fn get_recent_shrinks(
        &self,
        chat_id: &ChatIdKind,
        days: DaysCount,
    ) -> anyhow::Result<Vec<RecentShrink>> {
        let days = days.value() as i32;
        sqlx::query_as!(RecentShrink,
            r#"SELECT usr.name AS "owner_name: Username", s.lost_length AS "lost_length!: Length"
                FROM Stale_Dick_Shrinks s
                JOIN Users usr USING (uid)
                JOIN Chats c ON c.id = s.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND s.created_at > current_date - $2::int
                ORDER BY s.created_at DESC, s.lost_length DESC"#,
                chat_id.value() as String, days)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't fetch recent shrinks for {chat_id}"))
    }
);
