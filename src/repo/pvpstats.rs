use anyhow::Context;
use num_traits::{Num, ToPrimitive};
use sqlx::{FromRow, Postgres, Transaction};
use teloxide::types::UserId;
use crate::domain::objects::{BattleStats, LoserStats, UserStats, WinnerStats};
use crate::domain::primitives::{BattlesCount, Bet, Length, WinStreak};
use crate::domain::primitives::chat::InternalChatId;
use crate::repo::ChatIdKind;
use crate::repository;

#[derive(Default, FromRow)]
struct UserStatsEntity {
    battles_total: i32,
    battles_won: i32,
    win_streak_max: i16,
    win_streak_current: i16,
    acquired_length: i32,
    lost_length: i32,
}

#[derive(FromRow)]
struct UserBattlesStatsEntity {
    battles_total: i32,
    battles_won: i32,
}

impl Into<UserStats> for UserStatsEntity {
    fn into(self) -> UserStats {
        UserStats {
            battles_total: self.battles_total.to_u32().map(BattlesCount::new).expect("battles_total, fetched from the database, must not be negative"),
            battles_won: self.battles_won.to_u32().map(BattlesCount::new).expect("battles_won, fetched from the database, must not be negative"),
            win_streak_max: self.win_streak_max.to_u16().map(WinStreak::new).expect("win_streak_max, fetched from the database, must not be negative"),
            win_streak_current: self.win_streak_current.to_u16().map(WinStreak::new).expect("win_streak_current, fetched from the database, must not be negative"),
            acquired_length: self.acquired_length.to_u32().map(WinStreak::new).expect("acquired_length, fetched from the database, must not be negative"),
            lost_length: self.lost_length.to_u32().map(WinStreak::new).expect("lost_length, fetched from the database, must not be negative"),
        }
    }
}

repository!(BattleStatsRepo, with_(chats)_(Chats),
    pub async fn send_battle_result(&self, chat_id_kind: &ChatIdKind, winner_id: UserId, loser_id: UserId, bet: Bet) -> anyhow::Result<BattleStats> {
        let chat_id = self.chats.get_internal_id(chat_id_kind).await?;
        let mut tx = self.pool.begin().await?;
        let winner = update_winner(&mut tx, chat_id, winner_id, bet.into()).await?;
        let loser = update_loser(&mut tx, chat_id, loser_id, bet.into()).await?;
        tx.commit().await?;
        Ok(BattleStats { winner, loser })
    }
,
    pub async fn get_stats(&self, chat_id_kind: &ChatIdKind, user_id: UserId) -> anyhow::Result<UserStats> {
        sqlx::query_as!(UserStatsEntity, "SELECT battles_total, battles_won, win_streak_max, win_streak_current, acquired_length, lost_length FROM Battle_Stats \
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text) AND uid = $2",
            chat_id_kind.value() as String, user_id.0 as i64)
        .fetch_optional(&self.pool)
        .await
        .map(Option::unwrap_or_default)
        .map(UserStats::from)
        .context(format!("couldn't get the stats for {chat_id_kind} and {user_id}"))
    }
);

async fn update_winner(tx: &mut Transaction<'_, Postgres>, chat_id: InternalChatId, uid: UserId, bet: Bet) -> anyhow::Result<WinnerStats> {
    sqlx::query_as!(UserStatsEntity, "INSERT INTO Battle_Stats(uid, chat_id, battles_total, battles_won, win_streak_current, acquired_length) VALUES ($1, $2, 1, 1, 1, $3) \
                ON CONFLICT (uid, chat_id) DO UPDATE SET \
                    battles_total = Battle_Stats.battles_total + 1, \
                    battles_won = Battle_Stats.battles_won + 1, \
                    win_streak_current = Battle_Stats.win_streak_current + 1, \
                    acquired_length = Battle_Stats.acquired_length + $3 \
                RETURNING battles_total, battles_won, win_streak_max, win_streak_current, acquired_length, lost_length",
            uid.0 as i64, chat_id, bet)
        .fetch_one(&mut **tx)
        .await
        .map(WinnerStats::from)
        .context(format!("couldn't update the stats of the winner: {chat_id}, {uid}, {bet}"))
}

async fn update_loser(tx: &mut Transaction<'_, Postgres>, chat_id: InternalChatId, uid: UserId, bet: Bet) -> anyhow::Result<LoserStats> {
    let uid = uid.0 as i64;
    let prev_win_streak = sqlx::query_scalar!("SELECT win_streak_current FROM Battle_Stats WHERE chat_id = $1 AND uid = $2", chat_id, uid)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't fetch the win streak of the loser: {chat_id}, {uid}"))?
        .unwrap_or(0);
    let win_rate = sqlx::query_as!(UserBattlesStatsEntity, "INSERT INTO Battle_Stats(uid, chat_id, battles_total, battles_won, win_streak_current, lost_length) VALUES ($1, $2, 1, 0, 0, $3) \
                ON CONFLICT (uid, chat_id) DO UPDATE SET \
                    battles_total = Battle_Stats.battles_total + 1, \
                    win_streak_current = 0, \
                    lost_length = Battle_Stats.lost_length + $3 \
                RETURNING battles_total, battles_won",
            uid, chat_id, bet)
        .fetch_one(&mut **tx)
        .await
        .context(format!("couldn't update the stats of the loser: {chat_id}, {uid}, {bet}"))?;
    Ok(LoserStats::new(win_rate, prev_win_streak))
}
