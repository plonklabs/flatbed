# syntax=docker/dockerfile:1
#
# Build context is the flatbed repo root (see docker-compose.yml) because this
# example depends on ../../crates/flatbed by path.

FROM rust:1-bookworm AS builder

# Pinned to match /.flatc-version. Bump both together — flatc version drift
# changes the generated code byte-for-byte.
ARG FLATC_VERSION=25.9.23
RUN apt-get update && apt-get install -y --no-install-recommends curl unzip \
 && curl -fL "https://github.com/google/flatbuffers/releases/download/v${FLATC_VERSION}/Linux.flatc.binary.g++-13.zip" -o /tmp/flatc.zip \
 && unzip /tmp/flatc.zip -d /usr/local/bin \
 && chmod +x /usr/local/bin/flatc \
 && flatc --version \
 && rm -rf /var/lib/apt/lists/* /tmp/flatc.zip

WORKDIR /src
COPY . .
RUN cargo build --release --manifest-path examples/telemetry/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder \
  /src/examples/telemetry/target/release/flatbed-example-telemetry \
  /usr/local/bin/app
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --retries=5 \
  CMD curl -fsS http://localhost:8080/healthz || exit 1
CMD ["app"]
