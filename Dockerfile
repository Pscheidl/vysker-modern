FROM postgres:18-bookworm AS postgres-tools
FROM rust:1.98.1-bookworm AS source
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY Dockerfile.web Dockerfile.prebuilt ./
COPY src ./src
COPY migrations ./migrations
COPY seed-demo.sql ./
COPY demo ./demo
COPY config ./config
COPY tests ./tests
COPY scripts ./scripts

FROM source AS test
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-psycopg python3-bs4 tzdata && rm -rf /var/lib/apt/lists/*
COPY --from=postgres-tools /usr/lib/postgresql/18/bin/ /usr/local/bin/
COPY --from=postgres-tools /usr/lib/x86_64-linux-gnu/libpq.so.5* /usr/lib/x86_64-linux-gnu/
CMD ["./scripts/test.sh", "--smtp"]

FROM source AS build
RUN cargo build --locked --release --bin obec-api --bin obec-admin

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 obec && useradd --uid 10001 --gid obec --no-create-home obec \
    && mkdir -p /app/data && chown obec:obec /app/data
WORKDIR /app
COPY --from=build /app/target/release/obec-api /app/target/release/obec-admin ./
USER 10001:10001
ENV OBEC_ADRESA=0.0.0.0:3000
EXPOSE 3000
CMD ["./obec-api"]
