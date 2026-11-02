# syntax=docker/dockerfile:1
# ============================================================
# Distributed Proofreaders — production image
#
# Build:   docker build -t dproofreaders:latest .
# Run:     docker run --rm -p 8080:8080 \
#            -e DP_DATABASE_URL=mysql://... \
#            -v dp-data:/var/lib/dp dproofreaders:latest
#
# The runtime image is distroless: no shell, no package manager, no curl.
# `dp healthcheck` provides the probe that Docker HEALTHCHECK calls.
# ============================================================

# ============================================================
# Stage 1: Build frontend assets
# ============================================================
FROM node:24-alpine AS frontend-builder
WORKDIR /app

COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY assets/ ./assets/
COPY scripts/ ./scripts/
RUN npm run build

# ============================================================
# Stage 2: Build the Rust binary
# ============================================================
FROM rust:1.95-bookworm AS rust-builder
WORKDIR /app

# Cache dependency compilation separately from application sources. Cargo
# refuses to parse a workspace member without a target, so seed empty targets
# first; the real sources copied below overwrite them.
COPY Cargo.toml Cargo.lock ./
COPY crates/dp-core/Cargo.toml crates/dp-core/
COPY crates/dp-db/Cargo.toml crates/dp-db/
COPY crates/dp-web/Cargo.toml crates/dp-web/
RUN mkdir -p crates/dp-core/src crates/dp-db/src crates/dp-web/src \
    && touch crates/dp-core/src/lib.rs crates/dp-db/src/lib.rs \
             crates/dp-web/src/lib.rs crates/dp-web/src/main.rs
RUN cargo fetch --locked

# rounds.toml and charsets.toml are embedded with include_str! from
# crates/dp-core/src, so they arrive with the sources above.
COPY crates/ ./crates/
RUN cargo build --release --locked -p dp-web

# ============================================================
# Stage 3: Prepare the writable data directory with the right owner
# ============================================================
FROM debian:bookworm-slim AS dirs
RUN mkdir -p /var/lib/dp && chown 65532:65532 /var/lib/dp

# ============================================================
# Stage 4: Distroless runtime (non-root)
# ============================================================
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="dproofreaders" \
      org.opencontainers.image.description="Distributed Proofreaders Vietnamese proofing server" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

COPY --from=rust-builder /app/target/release/dp /usr/local/bin/dp
COPY --from=frontend-builder /app/static/ /srv/static/
COPY --from=dirs --chown=65532:65532 /var/lib/dp /var/lib/dp

# Distroless `nonroot` is UID/GID 65532. Never run the service as root.
USER 65532:65532
WORKDIR /var/lib/dp

ENV RUST_LOG="info,dp_web=info,dp_db=warn" \
    DP_LOG_FORMAT="json" \
    DP_BIND="0.0.0.0:8080" \
    DP_DATA_DIR="/var/lib/dp" \
    DP_MANIFEST_PATH="/srv/static/manifest.json"

EXPOSE 8080

# The binary implements the probe itself; no shell or curl in the image.
HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/dp", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/dp"]
CMD ["serve"]
