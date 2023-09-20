FROM rust:alpine as builder
WORKDIR /build

RUN apk update && apk add --no-cache pkgconfig musl-dev libressl-dev

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
COPY Cargo.* ./

ENV RUSTFLAGS='-C target-feature=-crt-static'
RUN cargo build --release && mv target/release/dick-grower-bot /dickGrowerBot

FROM alpine
RUN apk update && apk add --no-cache libgcc libressl
COPY migrations/ migrations/
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
ENTRYPOINT [ "/usr/local/bin/dickGrowerBot" ]

LABEL org.opencontainers.image.source=https://github.com/kozalosev/DickGrowerBot
LABEL org.opencontainers.image.description="Who has the biggest dick ever? A game bot for Telegram"
LABEL org.opencontainers.image.licenses=MIT
