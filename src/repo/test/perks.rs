use domain_types::literal;
use sqlx::types::JsonValue;
use crate::domain::objects::PerkStateUpdate;
use crate::domain::primitives::{LengthChange, PerkName};
use crate::repo;
use crate::repo::test::dicks::create_user;
use crate::repo::test::{CHAT_ID_KIND, fresh_db, USER_ID};

fn state_of(value: i64) -> JsonValue {
    serde_json::json!({ "value": value })
}

async fn insert_perk(db: &sqlx::Pool<sqlx::Postgres>, name: &str) -> anyhow::Result<()> {
    sqlx::query!("INSERT INTO Perks (name) VALUES ($1)", name)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
async fn registration_is_idempotent_and_keeps_the_ids() {
    let db = fresh_db().await;
    let perk_states = repo::PerkStates::new(db);
    let names = [literal!(PerkName = "first"), literal!(PerkName = "second")];

    let ids = perk_states.register_all(&names)
        .await.expect("couldn't register the perks");
    assert_eq!(ids.len(), 2);

    let again = perk_states.register_all(&names)
        .await.expect("couldn't register the perks a second time");
    assert_eq!(again, ids, "a second run must find the same ids");

    let with_a_new_one = {
        let names = [literal!(PerkName = "first"), literal!(PerkName = "second"), literal!(PerkName = "third")];
        perk_states.register_all(&names)
            .await.expect("couldn't register a new perk")
    };
    assert_eq!(with_a_new_one.len(), 3);
    assert_eq!(with_a_new_one[&names[0]], ids[&names[0]], "an old perk must keep its id");
}

/// The rule lives in two places — `perk_name_validator` and the column's `CHECK` — because the
/// constraint is what lets `register_all` turn a row back into a `PerkName` and know it will fit.
/// That only holds while the two say the same thing.
#[tokio::test]
async fn the_column_refuses_what_the_type_refuses() {
    let db = fresh_db().await;

    for name in ["", "серия", "🍆", &"a".repeat(33)] {
        assert!(PerkName::of(name).is_err(), "the type must refuse {name:?}");
        assert!(insert_perk(&db, name).await.is_err(), "the column must refuse {name:?}");
    }

    for name in ["help-pussies", "streak_2", &"a".repeat(32)] {
        assert!(PerkName::of(name).is_ok(), "the type must take {name:?}");
        assert!(insert_perk(&db, name).await.is_ok(), "the column must take {name:?}");
    }
}

#[tokio::test]
async fn a_state_is_written_by_the_growth_and_read_back() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let perk_states = repo::PerkStates::new(db.clone());
    create_user(&db).await;

    let name = literal!(PerkName = "first");
    let ids = perk_states.register_all(std::slice::from_ref(&name))
        .await.expect("couldn't register the perk");
    let perk_id = ids[&name];

    let empty = perk_states.read(USER_ID, &CHAT_ID_KIND)
        .await.expect("couldn't read the states of a dick without any");
    assert!(empty.of(perk_id).is_none());

    let update = PerkStateUpdate { perk_id, state: state_of(1) };
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(5), &[update])
        .await.expect("couldn't grow the dick");

    let stored = perk_states.read(USER_ID, &CHAT_ID_KIND)
        .await.expect("couldn't read the stored state");
    assert_eq!(stored.of(perk_id), Some(&state_of(1)));
    assert_eq!(stored.today, chrono::Utc::now().date_naive(),
        "the tests and the database must agree on the day, or the streak arithmetic is untestable");
}

/// The reason the states are written by the growth instead of by the perk: the second growth of the
/// day is refused, and nothing of what the perks decided may survive that.
#[tokio::test]
async fn a_refused_growth_stores_nothing() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let perk_states = repo::PerkStates::new(db.clone());
    create_user(&db).await;

    let name = literal!(PerkName = "first");
    let ids = perk_states.register_all(std::slice::from_ref(&name))
        .await.expect("couldn't register the perk");
    let perk_id = ids[&name];

    let first = PerkStateUpdate { perk_id, state: state_of(1) };
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(5), &[first])
        .await
        .expect("couldn't grow the dick")
        .expect("the first growth of the day must be allowed");

    let second = PerkStateUpdate { perk_id, state: state_of(2) };
    let refused = dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(5), &[second])
        .await.expect("the second growth must be refused, not fail");
    assert!(refused.is_none(), "the second growth of the day must be refused");

    let stored = perk_states.read(USER_ID, &CHAT_ID_KIND)
        .await.expect("couldn't read the stored state");
    assert_eq!(stored.of(perk_id), Some(&state_of(1)), "the refused growth must have changed nothing");
}
