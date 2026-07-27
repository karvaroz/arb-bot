# Multi-stage build: the final image is just the Rust binary + the built
# dashboard's static files + config/ — Axum serves both from one process
# (IMPLEMENTATION_PLAN.md Phase 5 deployment decision), no separate
# dashboard container.

# ---- dashboard build -------------------------------------------------
FROM node:22-bookworm-slim AS dashboard-build
WORKDIR /app/dashboard
COPY apps/dashboard/package.json apps/dashboard/package-lock.json ./
RUN npm ci
COPY apps/dashboard/ ./
# Same-origin in production (Axum serves both) — no cross-origin base URL
# needed, unlike the dev setup (Vite on :5173 talking to :8080).
ENV VITE_API_BASE=""
RUN npm run build

# ---- rust build --------------------------------------------------------
FROM rust:1-bookworm AS rust-build
# rusqlite's "bundled" feature compiles SQLite from C source; tokio-tungstenite
# pulls in native-tls, which needs OpenSSL headers to build against.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY apps/cli/ apps/cli/
RUN cargo build --release -p arb-bot-cli

# ---- runtime -------------------------------------------------------------
FROM debian:bookworm-slim
# ca-certificates: outbound HTTPS to Helius RPC / Jupiter API needs a real
# trust store, Debian's slim image doesn't ship one by default.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-build /build/target/release/arb-bot-cli ./arb-bot-cli
COPY --from=dashboard-build /app/dashboard/dist ./dashboard-dist
COPY config/ ./config/
EXPOSE 8080
ENTRYPOINT ["./arb-bot-cli"]
