# DickGrowerBot — Claude Code Guide

## Build & Run

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests (requires Docker for testcontainers)
cargo test

# Regenerate sqlx offline query cache
cargo sqlx prepare -- --tests

# Start via Docker Compose
docker-compose up
```

### Required environment variables (`.env`)

```
DATABASE_URL=postgres://...
TELOXIDE_TOKEN=...
```

Migrations run automatically on startup via `sqlx::migrate!`.

## Architecture

### Workspace layout

| Crate | Purpose |
|---|---|
| `DickGrowerBot` (root) | Main application binary |
| `domain_types` | Shared domain primitive types and traits |
| `domain_types_macro` | Proc-macro crate — `#[domain_type]` derive |

### Layer breakdown

```
config/      — env-var config structs, feature flags
domain/      — pure domain types (primitives, objects, traits, errors)
handlers/    — teloxide update handlers and business logic
repo/        — sqlx repository impls; DB access only
locales/     — rust-i18n translation files (YAML)
migrations/  — 21 SQL migration files, auto-applied on startup
```

### Key frameworks

- **teloxide** (custom fork) — Telegram bot framework
- **sqlx** — async, compile-time checked SQL queries; offline cache in `.sqlx/`
- **tokio** — async runtime
- **axum** — HTTP server (webhooks / health)
- **rust-i18n** — i18n via `locales/` YAML files

### Domain type macro system

`#[domain_type]` (from `domain_types_macro`) generates newtype wrappers with arithmetic impls, `From`/`Into`, sqlx `Type`/`Encode`/`Decode`, and other trait impls from a simple attribute annotation. See `domain_types/src/traits.rs` and `domain_types_macro/src/lib.rs`.

### Dependency injection

Repositories are grouped in a `Repositories` struct and injected into handlers via the `deps!` macro. Handlers do not construct repos directly.

### Feature toggles

Runtime features are gated by environment variables parsed in `config/`. Check `config/` for the list of flags.

## DB Migrations

Migration files live in `migrations/` (21 files, numbered sequentially). They are applied automatically at startup — no manual step needed in development.
