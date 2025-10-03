FROM rust:1.83-alpine3.21 as builder
WORKDIR /build

RUN apk update && apk add --no-cache musl-dev

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

COPY src/ src/
COPY locales/ locales/
COPY migrations/ migrations/
COPY .sqlx/ .sqlx/
COPY Cargo.* ./

ENV RUSTFLAGS='-C target-feature=-crt-static'
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
ARG HELP_PUSSIES_COEF
ARG LOAN_PAYOUT_COEF
ARG DOD_SELECTION_MODE
ARG DOD_RICH_EXCLUSION_RATIO
ARG ANNOUNCEMENT_MAX_SHOWS
ARG ANNOUNCEMENT_EN
ARG ANNOUNCEMENT_RU
### additional parameters of the Peezy fork
ARG ALLOWED_CHAT_ID
ARG CENTIMETERS_PER_EGGPLANT
ARG EGGPLANTS_MAX
###
ENTRYPOINT [ "/usr/local/bin/dickGrowerBot" ]

LABEL org.opencontainers.image.source=https://github.com/Peezy-BigD/PeezyBigDBot
LABEL org.opencontainers.image.description="Who has the biggest dick ever? A game bot for Telegram"
LABEL org.opencontainers.image.licenses='MIT+"Commons Clause" License Condition v1.0'
