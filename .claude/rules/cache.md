---
paths:
  - "src/cache.rs"
  - "src/dialogue.rs"
  - "src/config/cache.rs"
  - "src/config/caches.rs"
  - "src/handlers/utils/locks.rs"
  - "src/rate_limit.rs"
  - "docker-compose.override.yml"
  - "docker-compose.yml"
---

## The store of short-lived values

Everything the bot keeps briefly lives in `src/cache.rs`, which is the one place that keeps
anything: the three per-chat settings (`topics.rs`, `cleanup.rs`, the chat language in
`users/mod.rs`), the users fetched from user-service, what is known about the bot's rights in a
chat, the PVP locks (`handlers/utils/locks.rs`) and the dialogue states (`dialogue.rs`).

```
REDIS_HOST=localhost    # unset => the values are kept in this process
REDIS_PORT=6379
REDIS_PASSWORD=…
CACHE_MODE=REDIS        # REDIS | LOCAL | DISABLED; unset => REDIS with a host, LOCAL without one
```

**One store, three backends, and only one of them is a choice.** `Backend::Redis` and
`Backend::Local` hold the same keyspace — the rendered `CacheKey` — so a tenant never learns which
it got, and the fallback exists once instead of once per feature. `Local` is what an unset or
unreachable `REDIS_HOST` falls back to, so **the bot must start and run without a server**, which is
the path most likely to rot and so has a test of its own. `Disabled` is only ever asked for.

**The bot runs as a single instance, and `LOCAL` serves that perfectly well.** The issue's talk of
two instances describes what `LOCAL` could not do, not something that ever went wrong: with one
process, a map in that process is a correct cache and a correct lock. What `REDIS` buys a lone
instance is narrower and worth naming plainly — a `/promo` or `/support` dialogue that survives a
restart, and nothing else. Everything else it buys is insurance against a second instance that does
not exist yet.

The cost is on the other side of the same line: the topics gate runs on every command of every
group, and under `REDIS` each miss is a round trip where `LOCAL` had a `HashMap` lookup. Choose
accordingly, and don't read `LOCAL` as a degraded mode.

**Two tenants are not caches, and the switch doesn't reach them.** A cache may be turned off because
a miss costs a query and nothing else. A dialogue with nowhere to keep its state can never advance
past its first step, and a lock nobody holds is no lock — answering one offer twice is what the
guard is for, and not a thing to have a setting for. Both call `Cache::or_local()`, which turns a
disabled store into a local one and leaves the rest alone. So the `Local` store is always built,
whatever the mode.

That is why the locking has no switch of its own any more. `PVP_CALLBACK_LOCKS_ENABLED` was one, and
it earned its place while the lock was a `HashSet` with no expiry, where a leaked guard left a
battle unanswerable for as long as the process lived. `SET NX EX` frees itself after
`PVP_LOCK_TIME`, so the escape hatch guards nothing and only offers a way to put the bug
back. `BattleLocks` is a plain struct for the same reason: with nothing to switch and a store that
is always there, there was never a second variant to be.

**`PVP_LOCK_TIME` must outlive a handler, and that is the whole of it.** The guard frees the
lock as the handler ends, so the lifetime only bounds a process killed mid-battle. But if it runs
out while the handler is still working, the next answer goes straight through and the same attack is
resolved twice — which no release scheme can undo, since by then both are already running.

The bound is a handler's worst case: a few Telegram requests at `BOT_HTTP_TIMEOUT` plus
`pvp_impl_attack`'s four queries at `DATABASE_ACQUIRE_TIMEOUT` each. It is an estimate read
off the code, not a guarantee — nothing caps a handler's total time — so the default is 180 rather
than something snug. Generosity is cheap here: the restart after a death that leaves a lock behind
takes longer anyway.

**How long a value lives is not configured in `cache.rs`.** A lifetime belongs to the value, not to
the store: what makes a chat's language worth keeping for an hour says nothing about a lock that is
worth keeping for seconds. So every write takes one, and the numbers live together in `CachesConfig`
(`config/caches.rs`) with one variable each.

**Nothing there is a source of truth** (bar the two above). A miss is answered by the caller doing
the work again, so a server that is down, slow or simply absent costs only the work it would have
saved. Every failure is logged and swallowed: `Cache` returns no errors and never panics.

**A `Backend::Redis` that starts failing falls back for real, not just to a faster miss.** The first
failed call trips `RedisHealth::degraded` (`src/cache.rs`), and every call after that is routed by
`Backend::route` straight to `RedisState::fallback` — a local store `Backend::Redis` always carries
— without even attempting Redis. Only the map itself is built up front and free; its sweeper starts
lazily, on that same first trip, so a Redis that never fails never pays for a background task that
would spend its whole life finding an empty map. A lock taken in the fallback is a real lock, not a
`true` handed out because the store couldn't be asked; a dialogue written there is read back, not
lost until the outage ends. Trips on the *first* failure rather than after a few in a row:
`ConnectionManager` already retries internally with its own growing backoff before a call returns at
all, so waiting for several failures here would mean paying that backoff several times over, and
there is no correctness reason to wait — `RedisState::fallback` is what a single instance of the bot
already uses correctly under `CACHE_MODE=LOCAL`, not a degraded stand-in for `CACHE_MODE=REDIS`.

Recovery is a background probe, not the data path: `spawn_redis_health_check` pings Redis every
`REDIS_HEALTH_CHECK_INTERVAL` (a constant, on the same grounds as `SWEEP_INTERVAL` — it only decides
how promptly the bot notices Redis is back, not whether anything works while it waits) *only* while
degraded, and `RedisHealth::record` clears the flag on the first probe that succeeds. That same
successful probe drains `RedisState::fallback` and writes each live entry into Redis with whatever
is left of its original lifetime (`sync_fallback_to_redis`) — so a lock or a dialogue answered
mid-outage doesn't vanish the instant `Backend::route` stops reading from the fallback. It is
best-effort like everything else here: a write that fails is logged and given up on, not retried,
which for a lock is free (the same as never having been taken) and for a dialogue reads as if a
restart had just happened. Nothing keeps the fallback's copy once its write is attempted either way,
which is also what makes it empty — and its sweeper idle — the moment recovery is noticed, rather
than only once each entry's own TTL would have expired it anyway.

**`cache_fallback_active` follows the most recent call to Redis, not only the one at startup.**
`RedisHealth::record` sets it on every call that actually reaches Redis: `1` on failure, `0` on the
next success — which, once degraded, means only the health check's own probes touch it, since the
data path stops trying Redis entirely. `Cache::connect`'s own write is only the value it starts at.
So the gauge, and the alert built on it (`DickGrowerBotCacheFellBack`, `for: 5m` in server-configs),
cover a server that dies mid-session too, not only one that was never reached.

**A chat-keyed value spells the kind out** — `chat:id:-100…:topics`, not `chat:-100…:topics`. A
chat id and a chat instance are both signed 64-bit numbers, and `ChatIdKind`'s `Display` forwards to
the value, so two different chats would share an entry. `ChatIdKind::qualified` is what the keys
use, and `chat::test` pins it.

The local store expires lazily on read **and** is swept on a timer (`TASK_CACHE_SWEEPER`): a key
read once and never again would otherwise be kept for ever. One sweeper for every tenant, which is
what replaced the user-service cache's own. Its size is `cache_local_entries`, and it is the number
to watch, since it is the one that can grow without bound.

The client is `redis` with `ConnectionManager`, and there is deliberately **no pool**. Redis runs
commands one at a time, so extra connections buy no parallelism; the protocol multiplexes instead,
letting many requests share one socket, and `ConnectionManager` adds reconnection on top. A pool
would only add an acquire step that can fail, a size to choose, and a dead connection to retry.
Keep it this way unless something needs a connection to itself — `BLPOP`, `SUBSCRIBE`, `WATCH`.

**`MULTI`/`EXEC` is unsafe on a multiplexed connection; a Lua script is not.** An `EVAL` is one
command to the server and runs there without interleaving, so it is exactly as safe as any other
single command and is the way to do anything that takes more than one step. Both are in use:
`Cache::lock` is a plain `SET NX EX`, and `Cache::unlock` is a script, because "delete this key if
it still holds my token" cannot be one command and must not be two. A bare `DEL` would free
whatever holds the key — after an expiry, somebody else's lock — and that lets a third caller in
while the second is still working, which is the failure the lock exists to prevent.

The container in `docker-compose.yml` runs **Valkey**, the BSD-licensed fork. The protocol is Redis,
which is what the service, the network and the variables are named after; only the binaries differ
(`valkey-server`, `valkey-cli`). It sits behind the `redis` Compose profile, like `user-service` and
`tracing`.

**Every key begins with `KEY_PREFIX`** (`dgb:chat:id:-1001234:language`), and `Cache` applies it
rather than each key type, so a kind of value added later cannot be the one that forgets. A server
may be shared, and `chat:id:-1001234:language` says nothing about whose chat that is. The database
index in the URL isolates too; having both means neither is what everything rests on.

The user-service entry is prefixed like the rest, though the data behind it is the service's rather
than ours. Sharing it with another bot would mean agreeing on the encoding, which is a shape
invented here and read by nothing else — the store is where a saved round trip is kept, not where
two bots meet, and the service itself is that place. A user is stored as a tag byte and its encoded
message; the tag is what keeps "the service has no such user" apart from a user whose every field is
the default, which prost encodes to nothing at all.

## Dialogue state

`/promo` and `/support` have a second step, and their state used to sit in teloxide's
`InMemStorage`, so every restart dropped whatever conversation was in progress. `dialogue.rs`
implements teloxide's `Storage` over the store instead.

**Not teloxide's `RedisStorage`**, though the fork does carry the feature. That one is built on
`deadpool-redis`, which brings a second copy of the `redis` crate (0.32 beside our 1.5) and a
connection pool next to the multiplexed connection we deliberately don't pool; it keys by the bare
`chat_id`, which collides with anything else in a shared database; and it sets **no lifetime at
all**, so an abandoned dialogue would be kept for ever. The trait is three methods, so ours is
shorter than adopting that would have been, and it inherits the local fallback for free.

```
DIALOGUE_STATE_TIME=1h     # how long a half-finished command waits for its next message
```

That is not a staleness: it is how long the conversation stays open, and every answer starts it
again. `remove_dialogue` refuses when there was nothing to remove, which is what `InMemStorage` does
and what `Dialogue::exit` expects.

