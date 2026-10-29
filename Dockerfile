# Multi-stage build: React frontend, Rust backend, slim runtime image.
# One Axum process serves both the API and the built frontend on port 8080.

# --- Frontend -----------------------------------------------------------------
FROM node:22.12.0-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# --- Backend ------------------------------------------------------------------
FROM rust:1.90-bookworm AS backend
WORKDIR /build/backend
# Build dependencies first with placeholder sources so this layer is cached
# until Cargo.toml or Cargo.lock change. The placeholders mirror the real
# targets declared in Cargo.toml: src/lib.rs and the src/bin/main.rs binary.
COPY backend/Cargo.toml backend/Cargo.lock backend/build.rs ./
RUN mkdir -p src/bin && echo "fn main() {}" > src/bin/main.rs && touch src/lib.rs \
    && cargo build --release --locked --bin portfolio-investigation \
    && rm -rf src
COPY backend/ ./
# Touch the real sources so Cargo rebuilds them instead of reusing the
# placeholder artifacts (COPY can preserve older modification times).
RUN touch src/bin/main.rs src/lib.rs \
    && cargo build --release --locked --bin portfolio-investigation

# --- Runtime ------------------------------------------------------------------
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /app app \
    && mkdir -p /app/var \
    && chown app:app /app/var
WORKDIR /app
COPY --from=backend /build/backend/target/release/portfolio-investigation /usr/local/bin/portfolio-investigation
COPY --from=frontend /build/frontend/dist /app/static
# Reference data and manifest. The upload fixtures are copied too but are never
# imported automatically; they enter through the UI.
COPY data /app/data
# Configuration: read from the working directory (/app). INTROPIC_* environment
# variables override individual settings.
COPY backend/config.docker.toml /app/config.toml

# Logging uses the application's default filter (info, with SQLx at warn);
# set RUST_LOG to change it.

USER app
VOLUME ["/app/var"]
EXPOSE 8080
CMD ["portfolio-investigation", "/app/config.toml"]
