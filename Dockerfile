FROM rust:1.73 as builder

WORKDIR /usr/src/app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/app/target/release/lighter-bot /usr/local/bin/

WORKDIR /app
COPY config/ config/

CMD ["lighter-bot", "live", "--config", "config/settings.yaml"]
