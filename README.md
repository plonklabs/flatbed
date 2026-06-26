# flatbed

A small Rust HTTP framework for services that speak
[FlatBuffers](https://flatbuffers.dev/). You define your messages once in a
`.fbs` schema, flatbed generates the Rust types, and you write handlers that
take a typed request and return a typed response:

```rust
#[route("/ping")]
async fn handle_ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedError> {
    Ok(Response::ok(PingResponse { message: format!("pong: {}", req.body.message) }))
}
```

Built on [Hyper](https://hyper.rs/), routes are registered at compile time
(no router setup code, no reflection at runtime), the same handler serves both
JSON and binary FlatBuffer clients, and — if you opt in — an OpenAPI spec is
generated for you from the routes you declared.

[![crates.io](https://img.shields.io/crates/v/flatbed.svg)](https://crates.io/crates/flatbed)
[![docs.rs](https://img.shields.io/docsrs/flatbed)](https://docs.rs/flatbed)
[![CI](https://github.com/plonklabs/flatbed/actions/workflows/ci.yml/badge.svg)](https://github.com/plonklabs/flatbed/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## What flatbed is (and isn't)

flatbed is meant for **internal services that sit behind a proxy** — a reverse
proxy, API gateway, or service-mesh sidecar (Envoy, nginx, Linkerd, …). That
proxy is expected to terminate TLS and handle mTLS, certificates, and external
routing. So flatbed itself ships **no TLS at all** — no `rustls`, no
`native-tls` — and serves plain HTTP/1.1 or HTTP/2 cleartext (h2c), both
auto-negotiated. The result is a deliberately small framework: it does typed
request/response handling and route dispatch, and leaves transport security to
the layer that's already doing it.

If you need a server that terminates TLS itself and faces the public internet
directly, flatbed is not the right tool. If you're writing a service that runs
behind a gateway (very common in Kubernetes and service-mesh setups), it's a
good fit.

## Install

```bash
cargo add flatbed
```

For FlatBuffer codegen from `.fbs` schemas, also add `flatbed_build` to your
build dependencies.

### `flatbed` CLI

`flatbed` is a standalone FlatBuffer codegen tool. It takes a directory of
`.fbs` schemas and emits the Rust bindings flatbed services compile against —
`<stem>_generated.rs` (the FlatBuffer types) and `<stem>_flatbed.rs` (the
flatbed request/response glue) — plus the `.bfbs` reflection blobs.

Normally that codegen runs inside a `build.rs` via `flatbed_build::Config`. The
CLI does the same work **without compiling the workspace**, so you can
regenerate bindings by hand or run codegen as its own CI step (e.g. to check the
committed `_generated.rs` is still in sync with the schemas).

```bash
flatbed generate --schemas-dir ./schemas --out ./src/generated
```

It walks the top-level `.fbs` files in `--schemas-dir` (subdirectories like
`v1/` are pulled in via FlatBuffer `include` directives, not compiled as roots)
and writes the output into `--out`. The pinned `flatc` from `.flatc-version`
must be on `PATH`.

Install it either way:

```bash
# From crates.io, built locally (any platform):
cargo install flatbed_build

# Or a prebuilt Linux binary from the GitHub release (amd64 or arm64):
curl -fsSL -o flatbed \
  https://github.com/plonklabs/flatbed/releases/latest/download/flatbed-linux-amd64
chmod +x flatbed
```

Prebuilt binaries are published as
[GitHub release assets](https://github.com/plonklabs/flatbed/releases/latest),
not as GitHub Packages, each with a `.sha256` checksum file. Only Linux `amd64`
and `arm64` are prebuilt; on other platforms use `cargo install flatbed_build`.

## Quick example

```rust
use flatbed::{route, Flatbed, FlatbedConfig, FlatbedError, Request, Response};

#[route("/ping")]
async fn handle_ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedError> {
    Ok(Response::ok(PingResponse {
        message: format!("pong: {}", req.body.message),
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("ping").host("0.0.0.0").port(8080);
    Flatbed::run(config, |_| async { Ok(()) }).await
}
```

See [`crates/flatbed/README.md`](crates/flatbed/README.md) for the full API
walkthrough — route registration, request/response types, error handling,
telemetry, OpenAPI generation, and the boot lifecycle.

## How a request flows

**1. Schemas define your types.** You describe each message in a FlatBuffer
schema (`.fbs`), and codegen turns it into a Rust type:

```fbs
// schemas/ping.fbs
table PingRequest  { message: string; }
table PingResponse { message: string; }
```

Codegen normally runs in a `build.rs` (via `flatbed_build::Config`), or you can
run it by hand with the [`flatbed` CLI](#flatbed-cli). Either way you get the
Rust `PingRequest` / `PingResponse` types your handler names in
`Request<PingRequest>` and `Response<PingResponse>`.

**2. One handler, two wire formats.** flatbed picks the codec from the request's
`Content-Type` header:

| Request `Content-Type`         | Body is parsed as | Response is encoded as |
| ------------------------------ | ----------------- | ---------------------- |
| `application/json`             | JSON              | JSON                   |
| `application/x-flatbuffers`    | binary FlatBuffer | binary FlatBuffer      |

The **response always mirrors the request format**, so the same handler serves a
browser sending JSON and a service sending packed FlatBuffer bytes — you write
it once. A body-bearing request (POST/PUT/…) with neither content type is
rejected with `415 Unsupported Media Type`.

This is why FlatBuffers are a good fit here: the binary format is compact and
zero-copy for service-to-service traffic, while the JSON view keeps the same
endpoints easy to hit with `curl` or from a browser during development.

## OpenAPI (optional)

Enable the `openapi` feature and flatbed generates an
[OpenAPI 3](https://www.openapis.org/) document from the routes you've already
declared — no separate spec to hand-maintain. The `#[route]` macro captures each
request/response type's fields at compile time, and the server exposes:

- `GET /openapi.json` — the spec for the latest API version
- `GET /v{version}/openapi.json` — a specific version (routes can be tagged with
  a version; it defaults to `v1`)

Point Swagger UI, Redoc, or a client generator at those endpoints and they stay
in sync with the code automatically.

## Crates

| Crate | Purpose |
|---|---|
| [`flatbed`](crates/flatbed) | HTTP server, route registry, optional telemetry / OpenAPI / NATS / Kubernetes feature gates |
| [`flatbed_macros`](crates/flatbed_macros) | `#[route]`, `#[worker]`, and `#[flatbed::main]` procedural macros |
| [`flatbed_build`](crates/flatbed_build) | Build-time FlatBuffer codegen and the `flatbed` CLI tool (`cargo install flatbed_build` ships the binary) |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
