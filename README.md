[@DickGrowerBot](https://t.me/DickGrowerBot)
============================================

[![CI Build](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml/badge.svg?branch=main&event=push)](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml) [![@DickGrowerBot MAU](https://tgbotmau.quoi.dev/api/bot/DickGrowerBot/mau/badge?style=flat "@DickGrowerBot MAU")](https://tgbotmau.quoi.dev/?bot=DickGrowerBot)

A game bot for group chats that let its users grow their virtual "dicks" every day for some random count of centimeters (including negative values) and compete with friends and other chat members.

Additional mechanics
--------------------
_(compared with some competitors)_

* **The Dick of the Day** daily contest to grow a randomly chosen dick for a bit more.
* A way to play the game without the necessity to add the bot into a group (via inline queries with a callback button).
* Import from _@pipisabot_ and _@kraft28_bot_ (not tested! help of its users is required).
* PvP fights with statistics.

### Soon (but not very, I guess)
* an option to show mercy and return the award for the battle back;
* support for those who loses battles the most;
* more perks;
* achievements;
* referral promo codes;
* global monthly events;
* a shop.

Features
--------
* true system random from the environment's chaos by usage of the `get_random()` syscall (`BCryptGenRandom` on Windows, or other alternatives on different OSes);
* localizations for English, Russian, Italian, Persian, and Chinese (Simplified & Traditional), switchable per chat via `/language` — and per user too when the optional [user-service](#user-service-integration) is enabled;
* Prometheus-like metrics.

Technical stuff
---------------

### Requirements to run
* PostgreSQL;
* _\[optional]_ Docker (it makes the configuration a lot easier);
* _\[optional]_ [Task](https://taskfile.dev) to start everything with a single command (see _Running locally_ below);
* _\[optional]_ the [user-service](https://github.com/Kozalo-Blog/user-service) microservice for cross-bot user languages (see _user-service integration_ below);
* _\[for webhook mode]_ a frontal proxy server with TLS support ([nginx-proxy](https://github.com/nginx-proxy/nginx-proxy), for example).

### How to build the application?

`cargo build`/`cargo check` type-check SQL queries at compile time via `sqlx`. Unless
you're relying on the offline query cache (see below), this means your local database
schema must already match `migrations/` — `sqlx::migrate!` only applies migrations
automatically when the bot itself starts, not at build time. If a build fails with
confusing SQL-query type-mismatch errors after pulling or adding a migration, apply
pending migrations first (requires `sqlx-cli`, `cargo install sqlx-cli`):

```shell
cargo sqlx migrate run
```

### Running locally

The usual dev setup runs the **infrastructure in Docker** (PostgreSQL, and optionally the
`user-service`) while you run the **bot itself as a local binary**. `docker-compose.override.yml`
forwards the containers' ports to `localhost`, so the binary reaches PostgreSQL at
`localhost:5432` and the user-service at `localhost:${USER_SERVICE_GRPC_PORT}`.

Copy the example config first: `cp .env.example .env` (then fill in `TELOXIDE_TOKEN`).

With [Task](https://taskfile.dev) installed, one command brings the infra up, migrates the
database and starts the bot:

```shell
task run
```

| Task | What it does |
|------|--------------|
| `task infra` | Start the full infra — PostgreSQL + user-service — in Docker (ports forwarded to `localhost`); alias for `task infra:full` |
| `task infra:min` | Start only PostgreSQL (no user-service) |
| `task run` | `task infra`, apply migrations, then run the bot with `cargo run` |
| `task migrate` | Apply pending DB migrations (`cargo sqlx migrate run`) |
| `task up` | Build and start the **whole** stack (bot included) in Docker |
| `task down` | Stop and remove the Docker stack |
| `task infra:down` | Stop just the infra containers |

Without Task, the equivalent of `task run` is:

```shell
docker compose up -d --wait postgres user-service
cargo sqlx migrate run
cargo run
```

`docker-compose.override.yml` is loaded automatically by any `docker compose` command; it's
what forwards the container ports to `localhost` for the local binary. `task up`/`down` opt
out of it (via `-f docker-compose.yml`) to run the whole stack in Docker — but note that
running the **bot inside Docker** needs a Docker-oriented `.env` (`POSTGRES_HOST=postgres`,
and `GRPC_ADDR_USER_SERVICE=user-service:${USER_SERVICE_GRPC_PORT}` if the integration is on),
which is the opposite of the `localhost` values `.env.example` ships for the local-binary flow.

### [user-service](https://github.com/Kozalo-Blog/user-service) integration

user-service is a small gRPC microservice that stores a user's preferred interface language
and shares it across all of SadBot.Dev's bots. When enabled, the bot offers a personal
`/language` command in private chats and honours each user's saved language. When **disabled**,
languages come from Telegram as before and `/language` is hidden from private chats — the
chat-wide `/language` for group admins keeps working regardless, since it's stored in the
bot's own database.

`docker-compose.yml` bundles the `user-service` container. It shares the bot's PostgreSQL
server but uses its own `userservicedb` database, provisioned by
`postgres/init-user-service-db.sh` (which only runs on a **fresh** data volume — on an
existing `./data`, create the database once by hand; the script's header shows the command).

Enable the integration by setting `GRPC_ADDR_USER_SERVICE` in `.env`, choosing the host by
where the **bot** runs:

* **bot as a local binary** (infra in Docker): `GRPC_ADDR_USER_SERVICE=localhost:${USER_SERVICE_GRPC_PORT}`;
* **bot inside docker-compose**: `GRPC_ADDR_USER_SERVICE=user-service:${USER_SERVICE_GRPC_PORT}`.

See the `user-service` block in `.env.example` for the related variables (cache TTLs, gRPC
timeout, and the service's database credentials).

### How to rebuild .sqlx queries?
_(to build the application without a running RDBMS)_

```shell
cargo sqlx prepare -- --tests
```

### Adjustment hints

It's most probably you want to change the value of the `GROW_SHRINK_RATIO` environment variable to make the players upset and disappointed more or less often.

### How to disable a command?

Most of the command can be hidden from both lists: command hints and inline results. To do so, specify an environment variable like `DISABLE_CMD_STATS` (where `STATS` is a command key) with any value.
Don't forget to pass this variable to the container by adding it to the `docker-compose.yml` file!
