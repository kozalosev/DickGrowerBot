---
paths:
  - "src/handlers/dod.rs"
  - "src/repo/dicks.rs"
  - "src/config/app.rs"
  - "migrations/49_drop-dod-triggers.sql"
---

## Optional: how `WEIGHTS` picks the Dick of the Day

```
DOD_POOREST_BONUS_PERCENT=100  # α: the shortest is 1 + α times as likely to win as the longest; 0 => alike
DOD_RECENT_WINS_DAYS=5         # N: each win of the last N days divides the weight by one more; 0 => off
```

`DOD_SELECTION_MODE=WEIGHTS` draws among the active members with the weight
`(1 + α · (1 − percent_rank by length)) / (1 + wins in the last N days)`. The two halves pull in
opposite directions: α concentrates the wins on the short, N spreads them out again.

**The bonus is by rank, not by length**, so the ratio between the shortest and the longest is
exactly `1 + α` in every chat. A sigmoid over the length's z-score was used before, and a z-score
is scale-free: 948 against 1586 cm was as decisive as 522 against 5357, and the favourite took
1.4–1.5 times the uniform share in chats of every size. The bonus a win pays (a few dozen cm) can't
close a gap of hundreds, so nothing corrected it, and the same member won for weeks.

Raise α when the short don't catch up; lower it if the wins pile up on them. Raise N if the same
winner still repeats; lower it if the draw starts to feel like a queue — in a chat of two, a large N
is close to plain alternation.

## One Dick of the Day per chat per day

The same shape, one table over: `PRIMARY KEY (chat_id, created_at)` on `Dick_of_Day` **is** the
rule, and `Dicks::begin_dod` claims that key before it grows anybody. A conflict means the day is
already won, so the growth never happens and the winner's name is read back for the answer:
`begin_dod` returns `DickOfDayIntentStatus` — `Vacant(growth_tx)` or `AlreadyChosen(name)` — and only a vacant day
reaches `GrowthTx::commit_dod`, which returns `DickOfDayResult`, `Chosen` or `NoDick`.

Claiming first is what makes two `/dod`s in one chat serialise: the second waits on the index and
is told it lost. `trg_check_dod_timestamp` could not do that. Its `SELECT` for an existing winner
read past a concurrent election under READ COMMITTED, so the loser of a race got the primary key's
own `23505` instead of the trigger's `GD0E2`, and the chat was shown an internal error.

The name is read by **a second statement**, not by a `SELECT` beside the insert in one CTE: a
data-modifying CTE shares one snapshot with the query next to it, so the row the winning election
committed after ours began would not be visible there at all.

Two things went with the trigger (migration 49). `Dick_of_Day.created_at` is no longer forced to
today — the column's `DEFAULT current_date` is what the bot's own insert gets, and the chat merge
is the only caller that names a date, which is precisely what it wants; `Chats::move_dicks_of_the_day`
therefore no longer disables a trigger, and no longer holds an `ACCESS EXCLUSIVE` lock on the table
for the rest of the merge. And `trg_forbid_dod_updates` is gone, so the table is append-only only
because nothing in the bot updates it.

