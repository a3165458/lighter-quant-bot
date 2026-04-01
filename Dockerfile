# ═══════════════════════════════════════════════════════
# Lighter Quant Bot — Docker Build
# Multi-stage: build Rust binary + fetch signer .so
# ═══════════════════════════════════════════════════════

# ── Stage 1: Build the Rust binary ──
FROM rust:1.83-bookworm AS builder

WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY config/ config/
COPY benches/ benches/

RUN cargo build --release

# ── Stage 2: Fetch lighter-signer.so from PyPI package ──
FROM python:3.12-slim-bookworm AS signer

RUN pip install --no-cache-dir lighter-sdk && \
    cp /usr/local/lib/python3.12/site-packages/lighter/signers/lighter-signer-linux-amd64.so /tmp/lighter-signer.so || \
    cp /usr/local/lib/python3.12/site-packages/lighter/signers/lighter-signer-linux-arm64.so /tmp/lighter-signer.so

# ── Stage 3: Runtime image ──
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /usr/src/app/target/release/lighter-bot /usr/local/bin/lighter-bot

# Copy signer .so from signer stage
COPY --from=signer /tmp/lighter-signer.so /app/lighter-signer.so

# Copy config
COPY config/ /app/config/

# Create data directory for PnL persistence
RUN mkdir -p /app/data

# Expose dashboard port
EXPOSE 2028

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:2028/api/status || exit 1

# Default command: live trading
CMD ["lighter-bot", "live", "--config", "config/settings.yaml"]
