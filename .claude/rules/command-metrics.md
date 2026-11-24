---
paths:
  - "src/metrics.rs"
  - "src/handlers/info/mod.rs"
  - "src/handlers/privacy/mod.rs"
  - "src/handlers/utils/callbacks.rs"
  - "src/handlers/utils/command_filter.rs"
  - "src/handlers/utils/incrementor.rs"
  - "src/handlers/utils/locks.rs"
  - "src/handlers/utils/mod.rs"
  - "src/handlers/utils/self_destruction.rs"
  - "src/handlers/utils/tghack.rs"
---

## How a command is counted

`command_<name>_usage_total{state}` holds the phases of a command and nothing else: `invoked`
when it arrives and, for the commands with a second step, `finished` when that step happens. The
dashboard's rate panel reads `state!="finished"` as "invocations", so a `state` that means anything
else breaks it.

A refusal, where the user is told what to change and nothing is done, is a reason of
`command_rejected_total{command, reason}`, one metric for every command; the `command` values are
the `<name>` of the usage counters, so the two join. `/import` alone keeps
`command_import_rejected_total{reason, bot}`, because no other command has a `bot` and a metric's
labels are the same on every series. The reasons are one enum, `metrics::RejectionReason`, so
the same refusal is spelled the same in every command (`admins_only`, `another_user`,
`feature_disabled`). Each counter lists the reasons its command can give, and exports them at zero
from startup, so an alert's `increase()` sees the first one; a debug build asserts that nothing
else is counted. The user's own
decline (a loan's Refuse, a promo's Cancel) is not a refusal; neither is an empty `/top`, nor the
dispatcher's gates, which have counters of their own.

A failure is an error returned to the dispatcher, and autometrics already counts it on every
handler: `function_calls_total{function="…_handler", result="error"}`. So an invocation that never
ended is `invoked − finished − rejected − failed`, and a refused path must never count `finished`
too.

A new command that can refuse gets a `RejectionCounter` with its reasons, adding to
`RejectionReason` only a refusal no other command gives, and joins the `Lazy::force` list in
`metrics::force_registration`.

