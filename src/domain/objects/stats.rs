use crate::domain::primitives::{BattlesCount, Length, Percentage, WinStreak};

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

    fn win_rate_formatted(&self) -> String {
        format!("{}%", self.win_rate_percentage())
    }
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
        return Percentage::literal(0)
    }
    let ratio = (battles_won / battles_total)
        .expect("battles_won <= battles_total, so the ratio is always within [0; 1]");
    ratio.percentage()
}
