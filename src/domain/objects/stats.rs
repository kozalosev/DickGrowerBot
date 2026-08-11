use crate::domain::primitives::{BattlesCount, Length, Percentage, WinStreak};
use domain_types::literal;

pub struct UserStats {
    pub battles_total: BattlesCount,
    pub battles_won: BattlesCount,
    pub win_streak_max: WinStreak,
    pub win_streak_current: WinStreak,
    pub acquired_length: Length,
    pub lost_length: Length,
}

pub type WinnerStats = UserStats;

pub struct LoserStats {
    pub win_rate_percentage: Percentage,
    pub prev_win_streak: WinStreak,
}

pub struct BattleStats {
    pub winner: WinnerStats,
    pub loser: LoserStats,
}

pub trait WinRateAware {
    fn win_rate_percentage(&self) -> Percentage;
}

impl WinRateAware for UserStats {
    fn win_rate_percentage(&self) -> Percentage {
        win_rate_percentage(self.battles_won, self.battles_total)
    }
}

impl WinRateAware for LoserStats {
    fn win_rate_percentage(&self) -> Percentage {
        self.win_rate_percentage
    }
}

impl LoserStats {
    pub fn new(battles_won: BattlesCount, battles_total: BattlesCount, prev_win_streak: WinStreak) -> Self {
        Self {
            win_rate_percentage: win_rate_percentage(battles_won, battles_total),
            prev_win_streak,
        }
    }
}

fn win_rate_percentage(battles_won: BattlesCount, battles_total: BattlesCount) -> Percentage {
    if battles_total.is_zero() {
        return literal!(Percentage = 0)
    }
    match battles_won / battles_total {
        Ok(ratio) => Percentage::from(ratio),
        Err(e) => {
            // battles_won > battles_total can only mean corrupted stats; don't crash the handler
            tracing::error!(battles_won = %battles_won, battles_total = %battles_total, error = %e, "an invalid win rate");
            literal!(Percentage = 100)
        }
    }
}
