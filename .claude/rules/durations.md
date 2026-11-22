---
paths:
  - "src/config/announcements.rs"
  - "src/config/app.rs"
  - "src/config/bot.rs"
  - "src/config/cache.rs"
  - "src/config/caches.rs"
  - "src/config/env.rs"
  - "src/config/help.rs"
  - "src/config/incrementor.rs"
  - "src/config/integrations.rs"
  - "src/config/mod.rs"
  - "src/config/rate_limit.rs"
  - "src/config/self_destruction.rs"
  - "src/config/shrink.rs"
  - "src/config/throttle.rs"
  - "src/config/toggles.rs"
---

## How a span of time is written

Every setting that names one takes a number and a unit — `250ms`, `30s`, `15m`, `1h`, `3d`. A bare
number is seconds, so a value written before this still means what it did.

`ms` is the only unit of two letters, and it ends in the same one as `s`, so `parse_duration` tries
it before looking at the last letter. Read the other way round, `5ms` would be five *seconds* — a
thousandfold error with nothing to show for itself, which is what `milliseconds_are_not_mistaken_for_seconds_or_minutes`
is there to prevent.

The unit is in the **value**, never in the name. A name that carried it (`BAN_LIST_REFRESH_SECONDS`)
had to be renamed to change the unit, and gave two places for the unit to be stated and so one for
them to disagree — `EnvDuration::minutes` on a `_SECONDS` variable compiled and was off by sixty.
`env_duration!` and `parse_duration` in `config/env.rs` are the whole of it, and `secs`/`mins`/
`hours`/`days` write the fallbacks beside them.

The one exception is `MSG_SELFDESTRUCT_DELAY_OPTIONS_MINUTES`, which keeps its suffix because it is
not a span: it is the list of minute counts `/cleanup` offers a chat, stored as minutes in the
`Chats.settings` jsonb and shown as minutes on the buttons. The same goes for the `*_DAYS` knobs
that are `DaysCount` — a count of days is what they mean, not a duration.

