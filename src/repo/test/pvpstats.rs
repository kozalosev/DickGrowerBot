use teloxide::prelude::{ChatId, UserId};
use testcontainers::clients;
use crate::repo;
use crate::repo::{ChatIdKind, ChatIdPartiality, WinRateAware};
use crate::repo::test::dicks::{create_dick, create_user, create_user_and_dick_2};
use crate::repo::test::{CHAT_ID, start_postgres, UID};

#[tokio::test]
async fn test_all() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let pvp_stats = repo::BattleStatsRepo::new(db.clone(), Default::default());

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));

    // create user and dick #2
    create_user(&db).await;
    create_dick(&db).await;
    let uid_1 = UserId(UID as u64);
    // create user and dick #2
    create_user_and_dick_2(&db, &ChatIdPartiality::Specific(chat_id.clone()), "User-2").await;
    let uid_2 = UserId(UID as u64 + 1);
    
    // get stats when no rows
    let stats = pvp_stats.get_stats(&chat_id, uid_1).await
        .expect("couldn't fetch stats");
    assert_eq!(stats.battles_total, 0);
    assert_eq!(stats.battles_won, 0);
    assert_eq!(stats.win_streak_current, 0);
    assert_eq!(stats.win_streak_max, 0);
    assert_eq!(stats.win_rate_percentage(), 0.00);
    
    // send the first battle to check insertions
    let stats = pvp_stats.send_battle_result(&chat_id, uid_1, uid_2).await
        .expect("couldn't send result of the first battle");
    assert_eq!(stats.winner.battles_total, 1);
    assert_eq!(stats.winner.battles_won, 1);
    assert_eq!(stats.winner.win_streak_current, 1);
    assert_eq!(stats.winner.win_streak_max, 1);
    assert_eq!(stats.winner.win_rate_percentage(), 100.0);
    assert_eq!(stats.winner.win_rate_formatted(), "100.00%");
    assert_eq!(stats.loser.win_rate_percentage, 0.00);
    assert_eq!(stats.loser.prev_win_streak, 0);

    // send the second battle to check updates
    let stats = pvp_stats.send_battle_result(&chat_id, uid_2, uid_1).await
        .expect("couldn't send result of the first battle");
    assert_eq!(stats.winner.battles_total, 2);
    assert_eq!(stats.winner.battles_won, 1);
    assert_eq!(stats.winner.win_streak_current, 1);
    assert_eq!(stats.winner.win_streak_max, 1);
    assert_eq!(stats.winner.win_rate_percentage(), 50.0);
    assert_eq!(stats.winner.win_rate_formatted(), "50.00%");
    assert_eq!(stats.loser.win_rate_percentage, 50.0);
    assert_eq!(stats.loser.prev_win_streak, 1);

    // send the third battle to test the getter again and check percentage rounding
    pvp_stats.send_battle_result(&chat_id, uid_2, uid_1).await
        .expect("couldn't send result of the first battle");
    let stats = pvp_stats.get_stats(&chat_id, uid_1).await
        .expect("couldn't fetch stats");
    assert_eq!(stats.battles_total, 3);
    assert_eq!(stats.battles_won, 1);
    assert_eq!(stats.win_rate_formatted(), "33.33%");
}
