# ============================================================
# Stage 1: Build frontend assets
# ============================================================
FROM node:24-alpine AS frontend-builder
WORKDIR /app
COPY package.json ./
RUN npm install
COPY assets/ ./assets/
COPY scripts/ ./scripts/
RUN npm run build

# ============================================================
# Stage 2: Build Rust binary
# ============================================================
FROM rust:1.95-bookworm AS rust-builder
WORKDIR /app

# Copy manifests
COPY Cargo.toml ./
COPY crates/dp-core/Cargo.toml crates/dp-core/
COPY crates/dp-db/Cargo.toml crates/dp-db/
COPY crates/dp-web/Cargo.toml crates/dp-web/

# Copy sources
COPY crates/ crates/

# Build release binary
RUN cargo build --release -p dp-web

# ============================================================
# Stage 3: Minimal production runtime
# ============================================================
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create unprivileged service user
RUN groupadd -g 1000 dp && \
    useradd -u 1000 -g dp -s /bin/false -m dp

# Create directories
RUN mkdir -p /var/lib/dp /srv/static /etc/dp && \
    chown -R dp:dp /var/lib/dp /srv/static /etc/dp

# Copy static assets and binary
COPY --from=frontend-builder /app/static/ /srv/static/
COPY --from=rust-builder /app/target/release/dp /usr/local/bin/dp

USER dp
WORKDIR /var/lib/dp

ENV RUST_LOG="info,dp_web=debug,dp_db=debug"
EXPOSE 8080

HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -f http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/dp"]
CMD ["serve"]
