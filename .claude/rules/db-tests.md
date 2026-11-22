---
paths:
  - "src/repo/test/announcements.rs"
  - "src/repo/test/bans.rs"
  - "src/repo/test/broadcasts.rs"
  - "src/repo/test/chats.rs"
  - "src/repo/test/deletions.rs"
  - "src/repo/test/dicks.rs"
  - "src/repo/test/import.rs"
  - "src/repo/test/loans.rs"
  - "src/repo/test/mentions.rs"
  - "src/repo/test/mod.rs"
  - "src/repo/test/perks.rs"
  - "src/repo/test/promo.rs"
  - "src/repo/test/pvpstats.rs"
  - "src/repo/test/shrinks.rs"
  - "src/repo/test/stats.rs"
  - "src/repo/test/users.rs"
  - "src/test_containers.rs"
  - "Taskfile.yml"
---

## Tests against the database

The whole test binary shares **one** Postgres container, and every test takes a **database of its
own** out of it:

```rust
let db = fresh_db().await;
```

That is the entire API (`src/repo/test/mod.rs`). Three things make it work, and each of them is
load-bearing:

* **A runtime of its own.** Every `#[tokio::test]` builds a runtime and tears it down when the test
  ends, and a sqlx pool dies with the runtime that created it — sharing a pool between tests fails
  with *"a Tokio 1.x context was found, but it is being shutdown"* as soon as the first test
  finishes. So the container and its maintenance pool live on the runtime in
  `src/test_containers.rs`, reached with `test_containers::spawn(...)`. Not `block_on`, which panics
  inside a runtime. The per-test pool is built on the test's own runtime and dies with it, which is
  fine.
* **A template database.** The migrations run once per run into `test_template`; each test's
  database is `CREATE DATABASE … TEMPLATE test_template`, which is far cheaper than replaying every
  migration 54 times.
* **A reused container.** It is marked `ReuseDirective::Always` and deliberately outlives the run,
  so the next run finds it instead of paying the startup again. It is *only* removed by
  `task test:clean`. Its databases are named `test_run<pid>_<n>` and the ones left by earlier runs
  are dropped at startup — one test binary at a time is assumed, which is how `cargo test` runs.

**Every shared container is one `SharedContainer`** (`src/test_containers.rs`), declared as a
`static` next to the tests that need it — Postgres, the cache and VictoriaLogs. A reusable container
is matched by its **labels**, which is why the service name is a constructor argument and has to
differ: labelled alike, the second request is handed the first container and fails on a port that
isn't there. `task test:clean` matches the label by key, so it sweeps every value.

This replaced one container per test: ~35s for the suite instead of ~75s. Don't reintroduce a
per-test or per-file container, and don't put the shared pool in a plain `static` without the
runtime — both were tried and both fail in the ways described above.

