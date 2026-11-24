---
paths:
  - "src/telegram_observer.rs"
  - "src/errors/details.rs"
  - "src/errors/flat.rs"
  - "src/errors/handler.rs"
  - "src/errors/mod.rs"
  - "src/errors/moot.rs"
  - "src/handlers/utils/tghack.rs"
  - "src/domain/objects/tghack.rs"
  - "src/handlers/info/mod.rs"
  - "src/handlers/privacy/mod.rs"
  - "src/handlers/utils/callbacks.rs"
  - "src/handlers/utils/command_filter.rs"
  - "src/handlers/utils/incrementor.rs"
  - "src/handlers/utils/locks.rs"
  - "src/handlers/utils/mod.rs"
  - "src/handlers/utils/self_destruction.rs"
  - "src/handlers/utils/tghack.rs"
  - "src/config/bot.rs"
---

## Observing the Telegram Bot API calls

The outgoing requests are watched by `TelegramObserver` (`src/telegram_observer.rs`), attached to
the bot in `config/bot.rs`. It implements `RequestObserver`, a hook **our teloxide fork** adds to
`Bot` (`crates/teloxide-core/src/observer.rs`), and the hook is meant to be contributed upstream.
The `teloxide` dependency in `Cargo.toml` points at the fork's `forked-master`: upstream's `master`
plus what the bot needs before upstream has it, merged in from `feature/request-observer` and from
upstream pull requests that are still open.

Each request produces three things:

* `telegram_request_duration_seconds{method,outcome}` — how long the call took. The failed calls
  are measured too, so a timeout (the slowest case there is) is in the histogram rather than
  missing from it. `outcome` is `ok` or one of the kinds `telegram_request_errors_total` uses.
* `telegram_request_errors_total{kind}` — the failures of that same call, from that same
  `errors::handler::classify`. One classification feeds both, so the two can't disagree. It is
  counted **here** rather than at the dispatcher's error handler, which is the only place that sees
  requests at all: the schedulers never reach a dispatcher, so a broadcast-wide outage left the
  counter flat. The observer sits under every adaptor, so it sees them and polling's `getUpdates`
  alike. One consequence worth knowing: `Throttle` retries after a flood wait and the observer is
  below it, so `rate_limited` counts every rejection where the handler only ever saw the last one.
  `ContextLoggingErrorHandler` now only logs.
* a `telegram_request` client span, so a slow API call is a child span of the handler's trace
  instead of an unattributed gap inside it.
* when the API answers with `ApiError::Unknown` — its way of saying it disliked the payload without
  saying which part — an `error` record carrying the serialized request body. That is the only way
  to find out which entity or which over-long text was rejected.

The observer sits *below* the adaptors, so the time `Throttle` holds a request back is not counted
as request time. A multipart request (the file-uploading methods) is a stream by then and has no
body to log; it is still measured.

Changing any of this means updating the Grafana dashboard in the server-configs repo, next to the
"Telegram API request errors by kind" panel.

## A failure that leaves nothing to do

Some of Telegram's refusals mean the request is already moot, and `errors::moot::Moot` names them:

| `kind` | Telegram says | Which is |
|---|---|---|
| `stale_query` | the callback or inline query is too old | a press answered after the client stopped waiting — an outage's backlog, a slow handler |
| `not_modified` | the edit changes nothing | a button pressed twice |
| `message_gone` | the message to edit or delete is not there | deleted by somebody, or by its own self-destruction, while a button on it was pressed |

**A handler carries on past them**: every answer to a query and every edit or deletion in
`handlers/` awaits through `IgnoreMoot::ignore_moot`, which turns them into `None` and logs at
`debug`. Without it the first of them ended the handler — an edit of a deleted message left the
query unanswered and its button spinning, and the update was logged as a failure. A new request of
those kinds goes through it too; a `send_message` does not, since none of the three can come back
from one.

They are still counted, as kinds of `telegram_request_errors_total` of their own rather than as
`api`, which therefore stays the count of requests that were really wrong. A few of them are normal;
a burst of `stale_query` says updates arrive late.

## An id of an unexpected shape

The chat of an inline message is read out of its `inline_message_id` (`handlers/utils/tghack.rs`),
a format Telegram never documented. **A change to it has to be noticed the day it happens**, so
anything that doesn't decode is refused, logged at `error!` with the raw id, and counted in
`unexpected_telegram_ids_total{source="inline_message_id"}`; `DickGrowerBotUnexpectedTelegramId`
in server-configs fires on the first one. That includes a peer id no channel can have (past
10¹² − 2³¹): it means the bytes aren't laid out the way they are read, so nothing decoded from them
is trusted, the message id included. `resolve_inline_message_id` is the one place that logs it; its
callers fall back without logging again.

The throttle is the backstop. teloxide's `ChatId::to_bare` panics on an id outside every range
Telegram assigns, and the throttle's worker used to call it for each request it sent: one such id
killed the worker, the requests waiting in it panicked, and the bot went on sending unthrottled
until a restart. The fork now throttles such a request like a private chat, logs
`Malformed chat id …`, and reports it as `ThrottledChatKind::Malformed`, which the bot counts as
`source="outgoing_chat_id"` under the same alert.

