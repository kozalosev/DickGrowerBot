---
paths:
  - "src/config/throttle.rs"
  - "src/main.rs"
  - "src/metrics.rs"
  - "src/scheduler/broadcasts.rs"
---

## One throttle for everything

Every request the bot sends goes through **one** teloxide `Throttle`: the answers to users, the
shrink broadcast and the deletion worker alike. `ThrottleConfig::wrap` (`config/throttle.rs`) builds
it once in `main.rs`, and everything else receives a clone of that `ThrottledBot`. The only thing
that keeps the raw `Bot` is the update listener (`polling_default`, `axum_to_router`), whose
`getUpdates` and `setWebhook` the adaptor would pass straight through anyway. The dispatcher runs
`dptree::type_check` when it is built, so a handler that still asks for a plain `Bot` panics at
startup rather than quietly going around the throttle.

**There must be exactly one.** The adaptor counts requests in a worker task that the wrapper spawns,
and a clone only adds a handle to that same worker. Two wrappers would keep two separate histories,
each would allow the full budget, and together they would send twice as much.

**It only governs sending.** The adaptor throttles the message-*sending* methods and passes
everything else through: edits, deletions, callback answers and inline answers. So the deletion
worker is not bounded by it at all. That is teloxide's judgement, not an oversight: the documented
limits are about sending, and a deletion produces no message and no notification. What bounds the
worker is `MSG_SELFDESTRUCT_CONCURRENCY`. If that judgement is ever wrong, a 429 is not final: the
row is postponed and tried again, and it shows as `telegram_request_errors_total{kind="rate_limited"}`.

```
THROTTLE_MESSAGES_PER_SEC_OVERALL=30
THROTTLE_MESSAGES_PER_SEC_CHAT=1
THROTTLE_MESSAGES_PER_MIN_CHAT=20         # private chats and legacy groups
THROTTLE_MESSAGES_PER_MIN_SUPERGROUP=20   # every chat whose id starts with -100
```

**The limits are Telegram's own.** The overall one needs no reserve, since nothing is left outside
the throttle. Telegram's 30 a second is approximate and aimed at bulk sending, so a burst of answers
past it is held back for a second rather than refused. The per-minute limits are Telegram's 20 for a
group. A private chat never reaches that, because `RATE_LIMIT_USER_REQUESTS` already holds its only
member to 20 a window.

**A chat over its limit holds back only itself.** The fork's worker queues pending requests per chat
and walks the chats one by one. So a busy supergroup's answers at midnight wait for their own
minute, and every other chat's requests go past them. Upstream's worker kept them all in one queue
of `THROTTLE_MESSAGES_PER_SEC_OVERALL` slots, where a held-back request kept its slot and one group
could block the bot. That is why the supergroup limit used to be 60.

**A 429 pauses only the chat that got it.** The adaptor holds that chat's requests for the
`retry_after` and then retries the refused one, so the answer is late rather than lost. Only when
three chats are refused at the same time (`Settings::global_freeze_chats`, teloxide's default) does
it pause every chat, because that points to the bot-wide limit rather than to one group. Watch
`telegram_request_errors_total{kind="rate_limited"}`: a steady stream of those means the limits
should come down.

A hang *inside* the throttle (see `DAILY_SHRINK_BROADCAST_SEND_TIMEOUT` in `daily-shrink.md`) now
stops the answers to users too, not only the broadcast. Only the broadcast has a timeout around its
sends.

The supergroup limit's teloxide name (`messages_per_min_channel_or_supergroup`) is easy to misread.
The adaptor picks it whenever `ChatId::is_channel_or_supergroup()` holds, and that is only a check
that the id starts with -100. Supergroups pass it, so this limit governs most of the chats the bot
serves, while `THROTTLE_MESSAGES_PER_MIN_CHAT` is left for private chats and the few legacy groups.
The bot is never in a channel; the name is teloxide's.

`Settings::on_queue_full` is wired to `telegram_throttle_queue_full_total`. The worker calls it when
requests that their chats' limits would allow are still waiting, because
`THROTTLE_MESSAGES_PER_SEC_OVERALL` is used up. It does so at most once every 4 seconds, so the
counter counts moments, not requests. A few of them at midnight are normal: that is the shrink
broadcast. A number that keeps growing all day means the bot asks for more than the overall limit
allows, and the answers to users are waiting too.

**How long a request waited is `telegram_throttle_wait_seconds{chat_kind}`**, from
`Settings::on_wait`, which the fork calls for every request the worker lets through. `chat_kind`
is `private`, `group` or `supergroup`. `telegram_request_duration_seconds` can't show this wait,
because `TelegramObserver` sits below the adaptor and starts timing only once the throttle has let
the request go. The buckets separate no wait from a few hundred milliseconds, and those from the
seconds that a held-back chat waits.

