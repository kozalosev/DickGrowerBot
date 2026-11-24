---
paths:
  - "src/users/mock.rs"
  - "src/users/mod.rs"
  - "src/handlers/language.rs"
  - "src/config/integrations.rs"
  - "src/scheduler/broadcasts.rs"
  - "build.rs"
---

## Optional: user-service integration

The bot can integrate with the [user-service](https://github.com/Kozalo-Blog/user-service)
microservice (gRPC) to read/update a user's preferred language across all of Kozalo's bots:

```
GRPC_ADDR_USER_SERVICE=host:port   # unset => integration disabled, personal /language hidden in PMs
USER_CACHE_TIME=6m            # optional cache TTL for fetched users
```

`/language` is overloaded: in a private chat it changes the caller's personal language (via
user-service, above); in a group it sets a chat-wide language (admins only) that applies to
everyone and overrides each user's own preference. The chat-wide setting is stored in our own
`Chats.settings` (jsonb) column, so it works even when user-service is disabled:

```
CHAT_LANGUAGE_CACHE_TIME=1h   # optional TTL for the per-chat language cache (we own the data)
```

**The shrink broadcast resolves a language per chat, and that is the most expensive thing a summary
needs.** A chat with no language of its own falls back to a tally of what its players speak, which
costs a query *and* a call to the user-service — once per chat, on every chat that is owed a
summary. So both halves read through the store: the override goes through
`LanguageService::chat_language` rather than the repository beneath it, and the tally is kept under
a key of its own. Every chat is a miss the first night and a hit on the ones after, which is the
whole point.

```
MOST_POPULAR_LANGUAGE_SAMPLE_SIZE=100  # players the tally looks at
BROADCAST_LANGUAGE_CACHE_TIME=7d       # how long its answer is kept
```

The sample exists because a tally does not become truer for having every last member in it, while
the full roll of a large chat is a list the user-service then has to look up in one go. The lifetime
is far longer than the other per-chat settings because what it caches — which language a group
speaks — changes on the scale of months, and because it is the only one whose miss costs two
round trips instead of one. "No answer" is cached as readily as an answer: a chat whose players the
service has never heard of is the commonest case there is.

**The language is also written into the row** (`Scheduled_Shrink_Broadcasts.lang_code`, migration
45), and a claim that finds it there skips the resolving entirely. The cache and the column are not
the same thing: the cache is best-effort and shared, the column is durable and belongs to that one
summary. It rides along with `finish`/`postpone` rather than travelling in a statement of its own —
the row is being written anyway, and an extra round trip here would have spent in advance exactly
what the column saves. A `NULL` leaves what is stored alone, so an attempt that never got as far as
working the language out cannot erase one that did.

