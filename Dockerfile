FROM rust:1.96-alpine3.21 AS chef
WORKDIR /build
RUN apk update && apk add --no-cache musl-dev
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Create an unprivileged user
ENV USER=appuser
ENV UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    "${USER}"

ENV RUSTFLAGS='-C target-feature=-crt-static'

# Builds only the dependency graph, cached as its own layer as long as the
# manifests copied into `planner` above don't change — invalidated far less
# often than the actual source below.
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY src/ src/
COPY domain_types/ domain_types/
COPY domain_types_macro/ domain_types_macro/
COPY locales/ locales/
COPY migrations/ migrations/
COPY .sqlx/ .sqlx/
COPY Cargo.* ./
RUN cargo build --release && mv target/release/dick-grower-bot /dickGrowerBot

FROM alpine:3.21
RUN apk update && apk add --no-cache libgcc
COPY --from=builder /dickGrowerBot /usr/local/bin/
# Import the user and group files from the builder
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group
# Use the unprivileged user
USER appuser:appuser

EXPOSE 8080
ARG TELOXIDE_TOKEN
ARG RUST_LOG
ARG WEBHOOK_URL
ARG DATABASE_URL
ARG DATABASE_MAX_CONNECTIONS
ARG HELP_ADMIN_CHANNEL_RU
ARG HELP_ADMIN_CHANNEL_EN
ARG HELP_ADMIN_CHAT_RU
ARG HELP_ADMIN_CHAT_EN
ARG HELP_GIT_REPO
ARG CHATS_MERGING_ENABLED
ARG TOP_UNLIMITED_ENABLED
ARG MULTIPLE_LOANS_ENABLED
ARG PVP_DEFAULT_BET
ARG PVP_CHECK_ACCEPTOR_LENGTH
ARG PVP_CALLBACK_LOCKS_ENABLED
ARG PVP_STATS_SHOW
ARG PVP_STATS_SHOW_NOTICE
ARG GROWTH_MIN
ARG GROWTH_MAX
ARG GROW_SHRINK_RATIO
ARG GROWTH_DOD_BONUS_MAX
ARG NEWCOMERS_GRACE_DAYS
ARG TOP_LIMIT
ARG INACTIVITY_DAYS
ARG HELP_PUSSIES_COEF
ARG LOAN_PAYOUT_COEF
ARG DOD_SELECTION_MODE
ARG DOD_RICH_EXCLUSION_RATIO
ARG ANNOUNCEMENT_MAX_SHOWS
ARG ANNOUNCEMENT_EN
ARG ANNOUNCEMENT_RU
ARG ANNOUNCEMENT_IT
ARG ANNOUNCEMENT_FA
ARG ANNOUNCEMENT_ZH
ENTRYPOINT [ "/usr/local/bin/dickGrowerBot" ]

LABEL org.opencontainers.image.source=https://github.com/kozalosev/DickGrowerBot
LABEL org.opencontainers.image.description="Who has the biggest dick ever? A game bot for Telegram"
LABEL org.opencontainers.image.licenses='MIT+"Commons Clause" License Condition v1.0'
