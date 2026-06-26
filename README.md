# flatbed

A Rust HTTP framework with FlatBuffer codegen, built on Hyper. Designed for
Kubernetes pods sitting behind an Envoy sidecar — no TLS, no rustls, no
native-tls; the proxy handles all of that. Routes are declared with a
`#[route]` proc-macro, validated at compile time via the
[`inventory`](https://crates.io/crates/inventory) crate, and dispatched by
the built-in router.

[![crates.io](https://img.shields.io/crates/v/flatbed.svg)](https://crates.io/crates/flatbed)
[![docs.rs](https://img.shields.io/docsrs/flatbed)](https://docs.rs/flatbed)
[![CI](https://github.com/plonklabs/flatbed/actions/workflows/ci.yml/badge.svg)](https://github.com/plonklabs/flatbed/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## Install

```bash
cargo add flatbed
```

For FlatBuffer codegen from `.fbs` schemas, also add `flatbed_build` to your
build dependencies.

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
