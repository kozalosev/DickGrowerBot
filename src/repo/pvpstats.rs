use num_traits::{Num, ToPrimitive};
use sqlx::{FromRow, Postgres, Transaction};
use teloxide::types::UserId;

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

pub trait WinRateAware {
    fn win_rate_percentage(&self) -> f64;
    
    fn win_rate_formatted(&self) -> String {
        format!("{:.2}%", self.win_rate_percentage())
    }
}

pub struct UserStats {
    pub battles_total: u32,
    pub battles_won: u32,
    pub win_streak_max: u16,
    pub win_streak_current: u16,
    pub acquired_length: u32,
    pub lost_length: u32,
}

impl WinRateAware for UserStats {
    fn win_rate_percentage(&self) -> f64 {
        win_rate_percentage(self.battles_won, self.battles_total)
    }
}

impl From<UserStatsEntity> for UserStats {
    fn from(value: UserStatsEntity) -> Self {
        Self {
            battles_total: value.battles_total.to_u32().expect("battles_total, fetched from the database, must not be negative"),
            battles_won: value.battles_won.to_u32().expect("battles_won, fetched from the database, must not be negative"),
            win_streak_max: value.win_streak_max.to_u16().expect("win_streak_max, fetched from the database, must not be negative"),
            win_streak_current: value.win_streak_current.to_u16().expect("win_streak_current, fetched from the database, must not be negative"),
            acquired_length: value.acquired_length.to_u32().expect("acquired_length, fetched from the database, must not be negative"),
            lost_length: value.lost_length.to_u32().expect("lost_length, fetched from the database, must not be negative"),
        }
    }
}

type WinnerStats = UserStats;

pub struct LoserStats {
    pub win_rate_percentage: f64,
    pub prev_win_streak: u16,
}

impl WinRateAware for LoserStats {
    fn win_rate_percentage(&self) -> f64 {
        self.win_rate_percentage
    }
}

impl LoserStats {
    fn new(user_battles_stats: UserBattlesStatsEntity, prev_win_streak: i16) -> Self {
        Self {
            win_rate_percentage: win_rate_percentage(user_battles_stats.battles_won, user_battles_stats.battles_total),
            prev_win_streak: prev_win_streak.to_u16().expect("prev_win_streak, fetched from the database, must not be negative")
        }
    }
}

pub struct BattleStats {
    pub winner: WinnerStats,
    pub loser: LoserStats,
}

repository!(BattleStatsRepo, with_(chats)_(Chats),
    #[tracing::instrument]
    pub async fn send_battle_result(&self, chat_id_kind: &ChatIdKind, winner_id: UserId, loser_id: UserId, bet: u16) -> anyhow::Result<BattleStats> {
        let chat_id = self.chats.get_internal_id(chat_id_kind).await?;
        let mut tx = self.pool.begin().await?;
        let winner = update_winner(&mut tx, chat_id, winner_id, bet.into()).await?;
        let loser = update_loser(&mut tx, chat_id, loser_id, bet.into()).await?;
        tx.commit().await?;
        Ok(BattleStats { winner, loser })
    }
,
    #[tracing::instrument]
    pub async fn get_stats(&self, chat_id_kind: &ChatIdKind, user_id: UserId) -> anyhow::Result<UserStats> {
        sqlx::query_as!(UserStatsEntity, "SELECT battles_total, battles_won, win_streak_max, win_streak_current, acquired_length, lost_length FROM Battle_Stats \
                WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text) AND uid = $2",
            chat_id_kind.value() as String, user_id.0 as i64)
        .fetch_optional(&self.pool)
        .await
        .map(Option::unwrap_or_default)
        .map(UserStats::from)
        .map_err(|e| e.into())
    }
);

#[tracing::instrument]
async fn update_winner(tx: &mut Transaction<'_, Postgres>, chat_id: i64, uid: UserId, bet: i32) -> anyhow::Result<WinnerStats> {
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
        .map_err(|e| e.into())
}

#[tracing::instrument]
async fn update_loser(tx: &mut Transaction<'_, Postgres>, chat_id: i64, uid: UserId, bet: i32) -> anyhow::Result<LoserStats> {
    let uid = uid.0 as i64;
    let prev_win_streak = sqlx::query_scalar!("SELECT win_streak_current FROM Battle_Stats WHERE chat_id = $1 AND uid = $2", chat_id, uid)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(0);
    let win_rate = sqlx::query_as!(UserBattlesStatsEntity, "INSERT INTO Battle_Stats(uid, chat_id, battles_total, battles_won, win_streak_current, lost_length) VALUES ($1, $2, 1, 0, 0, $3) \
                ON CONFLICT (uid, chat_id) DO UPDATE SET \
                    battles_total = Battle_Stats.battles_total + 1, \
                    win_streak_current = 0, \
                    lost_length = Battle_Stats.lost_length + $3 \
                RETURNING battles_total, battles_won",
            uid, chat_id, bet)
        .fetch_one(&mut **tx)
        .await?;
    Ok(LoserStats::new(win_rate, prev_win_streak))
}

fn win_rate_percentage<T: Num + Into<f64>>(battles_won: T, battles_total: T) -> f64 {
    if battles_total.is_zero() {
        return 0.0
    }
    battles_won.into() / battles_total.into() * 100.0
}
