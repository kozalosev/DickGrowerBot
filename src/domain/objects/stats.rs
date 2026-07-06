use num_traits::ToPrimitive;
use crate::domain::primitives::{BattlesCount, Length, Percentage, Ratio, WinStreak};

pub struct UserStats {
    pub battles_total: BattlesCount,
    pub battles_won: BattlesCount,
    pub win_streak_max: WinStreak,
    pub win_streak_current: WinStreak,
    pub acquired_length: Length,
    pub lost_length: Length,
}

impl WinRateAware for UserStats {
    fn win_rate_percentage(&self) -> Percentage {
        win_rate_percentage(self.battles_won, self.battles_total)
    }
}

pub trait WinRateAware {
    fn win_rate_percentage(&self) -> f64;
    
    fn win_rate_formatted(&self) -> String {
        format!("{:.2}%", self.win_rate_percentage())
    }
}

pub type WinnerStats = UserStats;

pub struct LoserStats {
    pub win_rate_percentage: Percentage,
    pub prev_win_streak: WinStreak,
}

impl WinRateAware for LoserStats {
    fn win_rate_percentage(&self) -> f64 {
        self.win_rate_percentage
    }
}

impl LoserStats {
    fn new(user_battles_stats: UserBattlesStatsEntity, prev_win_streak: WinStreak) -> Self {
        Self {
            win_rate_percentage: win_rate_percentage(user_battles_stats.battles_won, user_battles_stats.battles_total),
            prev_win_streak: prev_win_streak.to_u16().map(WinStreak::new).expect("prev_win_streak, fetched from the database, must not be negative")
        }
    }
}

pub struct BattleStats {
    pub winner: WinnerStats,
    pub loser: LoserStats,
}

fn win_rate_percentage(battles_won: BattlesCount, battles_total: BattlesCount) -> Percentage {
    if battles_total.is_zero() {
        return Percentage::literal(0)
    }
    let ratio = Ratio::from(battles_won / battles_total);
    ratio.percentage()
}
