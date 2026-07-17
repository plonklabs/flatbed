# syntax=docker/dockerfile:1
#
# Build context is the flatbed repo root (see docker-compose.yml) because this
# example depends on ../../crates/flatbed by path.

FROM rust:1-bookworm AS builder

# flatc version drift changes the generated code byte-for-byte — keep this pinned.
ARG FLATC_VERSION=25.9.23
RUN apt-get update && apt-get install -y --no-install-recommends curl unzip \
 && curl -fL "https://github.com/google/flatbuffers/releases/download/v${FLATC_VERSION}/Linux.flatc.binary.g++-13.zip" -o /tmp/flatc.zip \
 && unzip /tmp/flatc.zip -d /usr/local/bin \
 && chmod +x /usr/local/bin/flatc \
 && flatc --version \
 && rm -rf /var/lib/apt/lists/* /tmp/flatc.zip

WORKDIR /src
COPY . .
RUN cargo build --release --manifest-path examples/spa/Cargo.toml

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /src/examples/spa/target/release/flatbed-example-spa /usr/local/bin/app
# A relative static-mount dir resolves against the process working directory,
# so WORKDIR determines where the served files are read from.
COPY examples/spa/dist ./dist
EXPOSE 8080
CMD ["app"]
