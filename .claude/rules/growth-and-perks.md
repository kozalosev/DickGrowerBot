---
paths:
  - "src/handlers/dick.rs"
  - "src/handlers/perks.rs"
  - "src/handlers/utils/incrementor.rs"
  - "src/repo/dicks.rs"
  - "src/repo/perks.rs"
  - "src/repo/chats.rs"
  - "src/config/incrementor.rs"
  - "migrations/41_create-perk-states-tables.sql"
  - "migrations/48_drop-dicks-timestamp-trigger.sql"
---

## What counts as a growth

> `GrowthTx::commit_growth` is the growth, and the only one. It is subject to the once-a-day rule and
> it spends a bonus attempt. Every other write to `Dicks` is not a growth: it leaves `updated_at`
> alone and `bonus_attempts` is not its to move.

**The rule lives in that one statement** (`src/repo/dicks.rs`), in the `WHERE` of its
`ON CONFLICT DO UPDATE`: the row is rewritten only when the stored `updated_at` is of an earlier day
or a bought attempt can pay for it, and the same statement spends the attempt — **only when one was
needed**, so the day's first growth is free whatever the player has in store. A refusal is
therefore not an error but an empty answer — `commit_growth` returns `Option`, like `commit_dod`
beside it, and `handlers::dick` turns `None` into the "come back tomorrow" notice.

That is a change of kind, not only of place. `trg_check_and_update_dicks_timestamp` applied the rule
to **any** statement that set `updated_at`, so the rule held whoever wrote the row; now it holds
only for whoever goes through `commit_growth`. A growth written by hand somewhere else would not be
refused, and nothing in the schema would say so. What guards it instead is
`src/repo/test/dicks.rs` — the once-a-day refusal, a bonus attempt paying for a second growth, a
non-growth write spending none and not reopening the day — and this paragraph.

The trigger was worth removing because it had grown into a tax on everything around it. Its second
half served `bonus_attempts`, which nothing in the bot ever grants, so the chat merge
(`src/repo/chats.rs`) carried two `+1`s whose only purpose was to cancel a decrement it never
wanted; three tests dropped the trigger outright to be able to write what they meant; and the
refusal travelled as SQLSTATE `GD0E1`, spelled out in the handler and unpacked from
`sqlx::Error::Database` twelve lines at a time.

Nothing about the row lock changes: `ON CONFLICT DO UPDATE` takes it before the `WHERE` is
evaluated, so two `/grow`s racing for the same dick serialise exactly as they did.

## Perks and the storage they are given

A perk changes a length change: `handlers/perks.rs` holds them, `perks::all` registers them, and
each is switched off by a `DISABLE_<NAME>` variable named after it. **A perk is a plugin**, so it
never gets a column of its own on `Dicks`. It gets `Perk_States` instead (migration 41) — a row per
`(chat, user, perk)` whose `state` is jsonb the perk alone understands. Nothing validates that blob:
its shape belongs to the perk, and a `CHECK` would have to name each perk's private shape to say
anything useful. `serde` is the schema, and a blob it can't read is treated as a fresh start rather
than a failure, which is what lets a perk change its stored shape without a data migration. Adding a perk therefore
needs no migration, only a row in `Perks`, and that name-to-id dictionary is read **once**, by
`Incrementor::new` at startup, in a single statement for all of them. Nothing looks a perk up by
name afterwards.

**The set of perks is closed: they are the variants of `perks::AnyPerk`**, not trait objects. That
is what lets `Perk::apply` be a native async method — no boxed future per call — and lets
`find_perk_config` find a perk with a `match` (`ConfigurablePerk::find_in`) rather than a downcast.
`enum_dispatch` writes the forwarding, so a new perk is a variant there and an entry in
`perks::all`. It awaits inside each arm only when the trait declares `apply` as a plain `async fn`;
with `-> impl Future + Send` every arm is a different future type and the `match` doesn't compile. `Incrementor` is generic over the perk type, defaulting to `AnyPerk`, only so that its
tests can run perks of their own.

**A growth is one transaction, and the perks write through it.** `Dicks::begin_growth` (or
`begin_dod`) opens a `GrowthTx`, which owns the transaction; `Incrementor::growth_increment` takes it
by `&mut`, hands the perks its connection as `PerkContext::conn`, and the caller ends it with
`GrowthTx::commit_growth` / `commit_dod`. Dropping a `GrowthTx` rolls everything back — which is what a
refused growth is: the once-a-day rule is the upsert in `commit_growth`, *after* the perks have
run, and a perk that wrote something (a loan payout, its state) leaves no trace when it says no.
The Telegram answer is sent after the commit, never inside the transaction.

**Every attempt is a savepoint.** When the perks cancel a roll out to zero,
`Incrementor::nonzero_increment` rolls its savepoint back — the perks' writes with it — and rolls
the base again, up to `MAX_REROLLS` times; if they still cancel it, the bare base is used and the
perks are skipped. A perk's state is written by the incrementor from `PerkOutcome::state`, inside
the attempt; anything else a perk needs to write goes through `ctx.conn` itself, as
`LoanPayoutPerk` does.

**A perk's error ends the attempt, and the perks with it.** `apply` returns a `Result` because a
failed statement poisons a Postgres transaction (`25P02`): nothing after it would work, so the
attempt is rolled back and the bare base is used — as it is for an `i64` overflow of the sum.

**The dick's row is locked first, the loan's second**, the same order `Loans::borrow` takes them
in (`begin_growth` locks the dick, `LoanPayoutPerk` locks the loan with `FOR UPDATE`). The payout
is therefore computed against the debt it will be taken from. Keep that order for any perk that
locks something of its own.

Two things a perk is handed rather than fetching itself: `ChangeSource`, because a Dick of the Day
award is not a growth and `dod_increment` shares the perk pipeline with `growth_increment`; and
`today`, which is the **database's** `current_date`, because that is the calendar the once-a-day
rule compares against. A perk counting days that asked this process's clock would count different
ones.

The streak perk (issue #156) is the first user of all this. It stores
`{"streak": …, "max": …, "last_grow": …}` and adds a centimetre for every **grade** the streak has
reached — `STREAK_BONUS_GRADES` lists the days they start on (`StreakGrades`). Only playing on a
*later* day advances the count — a second growth on the same day, bought with `bonus_attempts`, is
paid the same bonus and leaves it alone.

```
STREAK_BONUS_GRADES=2,4,8,16,32  # the day each +1 cm starts on; empty => the perk is off
```

Three things about that shape are deliberate:

* **The bonus is a length, not a share of the roll.** A share scales the swings together with the
  average, so a veteran's worst day would be twice a newcomer's and luck would outweigh the streak
  exactly as much as it did on day one. A fixed length moves the average and leaves the swings alone.
* **It is visible from the second day.** The perks' part of the answer is printed only when their
  change is non-zero, and a share of a small roll rounds to nothing — a bonus nobody sees teaches
  nobody how it works. The note names the next grade (`titles.perks.streak.next_grade`) for the
  same reason: an irregular step can't be learned by watching it.
* **The grades lengthen as they go**, so a long streak still has a next step without the bonus
  outgrowing the rolls.

**A shrink gets the same centimetres**, so a long enough streak cancels it: with the default list
and the default range, a month in a row outweighs the worst roll of −5. That is accepted as what a
month of daily play earns; inactivity still costs length through the daily shrink.

Unlike `help-pussies`, it is **on by default** (`PerksConfig::default`), so it works without a
change to server-configs.

`/stats` gets its line through `Perk::stats_line`, a hook with a default of `None`: the states are
read once and handed round, so a perk with nothing to say costs nothing.

