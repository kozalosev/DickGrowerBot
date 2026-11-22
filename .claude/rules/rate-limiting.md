---
paths:
  - "src/rate_limit.rs"
  - "src/config/rate_limit.rs"
  - "src/handlers/mod.rs"
  - "src/handlers/start.rs"
  - "src/main.rs"
---

## Optional: rate limiting

```
RATE_LIMIT_USER_REQUESTS=20   # requests per window per user; 0 => off
RATE_LIMIT_CHAT_REQUESTS=60   # requests per window per chat; 0 => off
RATE_LIMIT_WINDOW=1m
RATE_LIMIT_WARNINGS=3         # requests over a limit that are still told so
SPAM_LIMIT_START_REQUESTS=1   # bare /start commands a chat is answered per window; 0 => off
SPAM_LIMIT_START_WINDOW=10m
SPAM_LIMIT_PROMO_REQUESTS=30  # promo code attempts a user gets per window; 0 => off
SPAM_LIMIT_PROMO_WINDOW=1h
```

**A bare `/start` has a limit of its own, per chat.** Spam bots fire `/start` at the bot through
Telegram's guest mode, and the general limits let each of them have the full start message — so the
bot spammed the chat in return. Over `SPAM_LIMIT_START_REQUESTS` the command is dropped **without a
word**, unlike the general limit: a notice would be the very message this exists to suppress. A
promo deeplink (`/start promo-…`) is exempt, since that is how a channel's readers activate a code;
it stays under the general limits. The check is the first thing `start_cmd_handler` does, before
the language is resolved, and the dropped commands are counted in `start_spam_limited_total`.

This protects the bot against a flood. One account sending `/help` at 150 a second used up
Telegram's quota answering it, and every real user waited 20 seconds or got nothing.
`rate_limit::RateLimiter` counts each request twice, against the sender and against the chat, in a
fixed window kept in the store (`Cache::increment`: `INCR` plus an `EXPIRE` set only by the first
hit, as one Lua script). The counter that has gone further past its limit decides.

**Only what the bot would answer is counted**: our own commands (`checks::is_our_command`, the same
name check the topics gate uses), button presses and inline queries. A group where the bot reads
every message would otherwise use up its chatty members' allowance on messages the bot ignores. The
second step of a `/support` or `/promo` dialogue is a plain message, so it is free too. An inline
query has no chat, so only the user's counter applies to it.

**Only the first `RATE_LIMIT_WARNINGS` requests over a limit get an answer.** Each answer is a
request to Telegram, and a flood that is answered request by request spends the very quota the
limit exists to protect. The rest are dropped without a reply and counted in
`rate_limited_updates_total`. A button press or an inline query is answered with the window as its
`cache_time`, so Telegram answers the repeats itself and they never reach the bot.

**It fails open.** A store that can't be asked lets the request through. It uses
`Cache::or_local()`, as the locks do, so `CACHE_MODE=DISABLED` doesn't switch the protection off.

**The limits are read off a curve of what users do.** Each counter reports the count it has just
reached to a `metrics::ThresholdCurve`, `rate_limit_user_windows_total{reached}` and
`rate_limit_chat_windows_total{reached}`, so it costs no round trip beyond the one the limit already
makes. The series are a survival curve of **windows**: `reached="1"` is every window of a user (or a
chat) that had a counted request in it, and `reached="N"` is how many of them got to N or more. A
limit is right when the share at its threshold is tiny. The counters keep counting past the limit,
so what is reported above it shows how far a flood goes.

It counts windows, not users: someone flooding for ten minutes is ten windows, and how many
distinct people that is belongs to the logs. A limit of 0 switches its counter off and its curve
with it, so to measure before choosing, set the limit far above the last threshold rather than
to 0.

It comes right after the stale-command filter, above the setup message and `/topics`. A flood
has to be stopped before it costs a query, and it may sit there because it writes nothing for the
sender, which is the property the ban gate's ordering depends on.

## Optional: commands that arrive too late

```
COMMAND_MAX_AGE=10m  # how long a command may take to reach the bot; 0 => every one is answered
```

When the bot can't be reached, Telegram keeps the updates and delivers them all at once when it
can be reached again. A command that waited longer than this is dropped without a reply and counted
in `stale_commands_total`: during an outage people send a command several times, so answering the
backlog posts the same answer again and again, and a command dozens of minutes old is no longer
what anybody is waiting for. A blip of a minute or two stays under the limit and is answered late.

The age is `now − message.date`, so only messages are judged, and only our commands
(`checks::is_our_command`). A support reply, a dialogue step or a title change goes through
whatever its age, because nobody repeats those. A button press and an inline query carry no time
they were sent. A stale inline query is refused by Telegram anyway (`stale_query`), and a stale
press still does what it was pressed for.

It is the first branch of the dispatcher tree, above the rate limiter, so a backlog that is
dropped counts against nobody's window.

