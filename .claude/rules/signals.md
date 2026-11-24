---
paths:
  - "src/reload.rs"
  - "src/shutdown.rs"
  - "src/main.rs"
  - "Dockerfile"
---

## What each signal means

| Signal | Meaning | Handled in |
|---|---|---|
| `SIGHUP` | reload the announcements and the ban list | `reload.rs` |
| `SIGTERM` | stop, gracefully | `shutdown.rs` |
| `SIGINT` | the same, from a terminal | `shutdown.rs` |

**A signal cannot mean two things**, which is the whole reason this table exists. The `Dockerfile`
carried `STOPSIGNAL SIGHUP`, so `docker stop` — every deploy — asked the bot to *reload*. Nothing
stopped, Docker waited out its timeout and killed the process, and everything that only happens on a
clean exit never happened: the last batch of spans and log records was dropped, and
`BatchSpanProcessor`'s count of spans it had to drop (which is only reported at shutdown) was never
printed. `STOPSIGNAL` is gone, so Docker's default `SIGTERM` applies.

Both branches of `main` stop through the dispatcher's `ShutdownToken`, not through teloxide's
`enable_ctrlc_handler` — that one listens for Ctrl-C alone, and a container is never stopped with
Ctrl-C. The webhook branch had no handler at all, so production had nothing listening for anything.

**Stopping the dispatcher is the only lever needed**, because it stops its update listener too, and
that is what the webhook server's graceful shutdown waits on. So one token drains the whole thing in
order: no new updates, then the ones in flight, then the HTTP server, then `Telemetry::shutdown`
flushes what the exporters still hold. The polling branch has no such flag on its metrics server, so
that one listens for the signal itself — every listener of a signal receives it, so the two do not
compete.

