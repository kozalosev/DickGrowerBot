use anyhow::Context;
use num_traits::ToPrimitive;
use sqlx::FromRow;
use crate::domain::objects::{BattleStats, LoserStats, UserStats};
use crate::domain::primitives::{BattlesCount, Bet, Length, UserId, WinStreak};
use crate::domain::primitives::chat::InternalChatId;
use crate::repo::ChatIdKind;
use crate::repository;

#[derive(Default, FromRow)]
struct UserStatsEntity {
    battles_total: i32,
    battles_won: i32,
    win_streak_max: i16,
    win_streak_current: i16,
    acquired_length: i64,
    lost_length: i64,
}

/// The winner's full stats, the loser's battle counts, and the loser's win streak before this
/// battle. `loser_prev_win_streak` is `None` when the loser has no earlier record.
struct BattleResultEntity {
    winner_battles_total: i32,
    winner_battles_won: i32,
    winner_win_streak_max: i16,
    winner_win_streak_current: i16,
    winner_acquired_length: i64,
    winner_lost_length: i64,
    loser_battles_total: i32,
    loser_battles_won: i32,
    loser_prev_win_streak: Option<i16>,
}

impl TryFrom<UserStatsEntity> for UserStats {
    type Error = anyhow::Error;

    fn try_from(entity: UserStatsEntity) -> anyhow::Result<Self> {
        Ok(Self {
            battles_total: entity.battles_total.to_u32().map(BattlesCount::new)
                .context("battles_total, fetched from the database, must not be negative")?,
            battles_won: entity.battles_won.to_u32().map(BattlesCount::new)
                .context("battles_won, fetched from the database, must not be negative")?,
            win_streak_max: entity.win_streak_max.to_u16().map(WinStreak::new)
                .context("win_streak_max, fetched from the database, must not be negative")?,
            win_streak_current: entity.win_streak_current.to_u16().map(WinStreak::new)
                .context("win_streak_current, fetched from the database, must not be negative")?,
            acquired_length: Length::new(entity.acquired_length),
            lost_length: Length::new(entity.lost_length),
        })
    }
}

impl TryFrom<&BattleResultEntity> for UserStats {
    type Error = anyhow::Error;

    fn try_from(row: &BattleResultEntity) -> anyhow::Result<Self> {
        UserStatsEntity {
            battles_total: row.winner_battles_total,
            battles_won: row.winner_battles_won,
            win_streak_max: row.winner_win_streak_max,
            win_streak_current: row.winner_win_streak_current,
            acquired_length: row.winner_acquired_length,
            lost_length: row.winner_lost_length,
        }.try_into()
    }
}

impl TryFrom<&BattleResultEntity> for LoserStats {
    type Error = anyhow::Error;

    fn try_from(row: &BattleResultEntity) -> anyhow::Result<Self> {
        let battles_total = row.loser_battles_total.to_u32().map(BattlesCount::new)
            .context("battles_total, fetched from the database, must not be negative")?;
        let battles_won = row.loser_battles_won.to_u32().map(BattlesCount::new)
            .context("battles_won, fetched from the database, must not be negative")?;
        let prev_win_streak = row.loser_prev_win_streak.unwrap_or(0).to_u16().map(WinStreak::new)
            .context("win_streak_current, fetched from the database, must not be negative")?;
        Ok(LoserStats::new(battles_won, battles_total, prev_win_streak))
    }
}

repository!(BattleStatsRepo, with_(chats)_(Chats),
    pub async fn send_battle_result(
        &self,
        chat_id_kind: &ChatIdKind,
        winner_id: UserId,
        loser_id: UserId,
        bet: Bet,
    ) -> anyhow::Result<BattleStats> {
        let chat_id = self.chats.get_internal_id(chat_id_kind).await?;
        let row = sqlx::query_as!(BattleResultEntity,
            r#"WITH loser_before AS (
                    SELECT win_streak_current FROM Battle_Stats WHERE chat_id = $1 AND uid = $3
                ),
                winner_upsert AS (
                    INSERT INTO Battle_Stats(uid, chat_id, battles_total, battles_won, win_streak_current, acquired_length)
                    VALUES ($2, $1, 1, 1, 1, $4)
                    ON CONFLICT (uid, chat_id) DO UPDATE SET
                        battles_total = Battle_Stats.battles_total + 1,
                        battles_won = Battle_Stats.battles_won + 1,
                        win_streak_current = Battle_Stats.win_streak_current + 1,
                        acquired_length = Battle_Stats.acquired_length + $4
                    RETURNING battles_total, battles_won, win_streak_max, win_streak_current, acquired_length, lost_length
                ),
                loser_upsert AS (
                    INSERT INTO Battle_Stats(uid, chat_id, battles_total, battles_won, win_streak_current, lost_length)
                    VALUES ($3, $1, 1, 0, 0, $4)
                    ON CONFLICT (uid, chat_id) DO UPDATE SET
                        battles_total = Battle_Stats.battles_total + 1,
                        win_streak_current = 0,
                        lost_length = Battle_Stats.lost_length + $4
                    RETURNING battles_total, battles_won
                )
                SELECT
                    w.battles_total AS "winner_battles_total!", w.battles_won AS "winner_battles_won!",
                    w.win_streak_max AS "winner_win_streak_max!", w.win_streak_current AS "winner_win_streak_current!",
                    w.acquired_length AS "winner_acquired_length!", w.lost_length AS "winner_lost_length!",
                    l.battles_total AS "loser_battles_total!", l.battles_won AS "loser_battles_won!",
                    lb.win_streak_current AS loser_prev_win_streak
                FROM winner_upsert w, loser_upsert l
                LEFT JOIN loser_before lb ON true"#,
                chat_id as InternalChatId, winner_id as UserId, loser_id as UserId, bet as Bet)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't send the battle result: {chat_id}, winner {winner_id}, loser {loser_id}, {bet}"))?;

        let winner = UserStats::try_from(&row)?;
        let loser = LoserStats::try_from(&row)?;
        Ok(BattleStats { winner, loser })
    }
,
    pub async fn get_stats(&self, chat_id_kind: &ChatIdKind, user_id: UserId) -> anyhow::Result<UserStats> {
        sqlx::query_as!(UserStatsEntity, "SELECT battles_total, battles_won, win_streak_max, win_streak_current, acquired_length, lost_length FROM Battle_Stats \
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text) AND uid = $2",
            chat_id_kind.value() as String, user_id as UserId)
        .fetch_optional(&self.pool)
        .await
        .context(format!("couldn't get the stats for {chat_id_kind} and {user_id}"))?
        .unwrap_or_default()
        .try_into()
    }
);
