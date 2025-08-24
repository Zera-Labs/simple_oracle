# syntax=docker/dockerfile:1

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build deps layer
RUN cargo chef cook --release --recipe-path recipe.json
# Build app
COPY . ./
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/zera_oracle /app/zera_oracle
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
# Persist DB to a mounted volume if provided
VOLUME ["/data"]
ENV ORACLE_DB_PATH=/data/oracle.sqlite
EXPOSE 8000
CMD ["/entrypoint.sh"] 