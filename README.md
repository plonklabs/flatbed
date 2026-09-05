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

# Or a prebuilt binary from the GitHub release. Detect your OS + arch:
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m); case "$ARCH" in x86_64) ARCH=amd64 ;; aarch64|arm64) ARCH=arm64 ;; esac
curl -fsSL -o flatbed \
  "https://github.com/plonklabs/flatbed/releases/latest/download/flatbed-${OS}-${ARCH}"
chmod +x flatbed
```

Prebuilt binaries are published as
[GitHub release assets](https://github.com/plonklabs/flatbed/releases/latest),
not as GitHub Packages, each with a `.sha256` checksum file. Linux and macOS
(`amd64` + `arm64` each) are prebuilt; on any other platform use
`cargo install flatbed_build`.

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

## TypeScript client

A service's OpenAPI spec (`/openapi.json`) and served FlatBuffer schema
(`/schema.bfbs`) are enough to generate a fully typed TypeScript client — no Rust
toolchain, no `flatc`. [`@plonklabs/flatbed-client`](clients/ts/flatbed-client)
reads both and emits typed methods, one per route:

```bash
npm i -D @plonklabs/flatbed-client
npx flatbed-client generate --server http://localhost:8080 --out src/api
```

```ts
import { createFlatbedClient } from "./api";

const api = createFlatbedClient({ baseUrl: "http://localhost:8080" });

const pong = await api.postPing({ body: { message: "hi" } });                     // FlatBuffer (default)
const pongJson = await api.postPing({ body: { message: "hi" } }, { as: "json" }); // JSON, per call
```

The generated client speaks the same two wire formats the server does. See the
[package README](clients/ts/flatbed-client) for the full walkthrough and
[`clients/ts/examples/openapi-consumer`](clients/ts/examples/openapi-consumer) for
a runnable end-to-end example. The [`production`](examples/production) example is
a deploy-shaped server — a business route plus telemetry and served reflection —
to point the generator at.

## Serving static files and a bundled SPA

A flatbed service can serve a built frontend (a Vite/React `dist/`, say)
alongside its JSON API from the **same origin** — one box, no CORS. Mount a
directory with `static_route!`:

```rust
// Declared #[route]s win; every other GET is served from dist/.
// Unknown non-API paths fall back to index.html for client-side routing.
flatbed::static_route!(mount = "/", dir = "/app/dist", fallback = "index.html");
```

Files are read from the container filesystem at request time, so ship the
directory in your image (e.g. `COPY dist/ /app/dist`). The `Content-Type` comes
from the file extension; `Cache-Control` is `no-cache` for HTML and other
stable-name files (`json`, `txt`, `ico`, `xml`, `webmanifest`), and
`public, max-age=31536000, immutable` for content-hashed assets. A missing path
*with* an extension is a real `404` (a broken asset URL isn't masked by the
shell); an extensionless miss serves the `fallback`. Declared routes always take
precedence, so `/api/*` keeps working under a `/` mount. (A configured
`splash` banner answers `GET /` ahead of a root mount — don't set both.)

For a handler that needs to return a body the JSON/FlatBuffer path can't express
— HTML, CSV, an image — `Response::raw` sets the bytes and `Content-Type`
directly:

```rust
#[route("/report.csv", method = "POST")]
async fn report(req: Request<ReportQuery>) -> Result<Response<()>, FlatbedRouteError> {
    Ok(Response::raw(build_csv(&req.body).into_bytes(), "text/csv; charset=utf-8"))
}
```

The [`raw-response`](examples/raw-response) example returns CSV and SVG from
handlers; the [`static-assets`](examples/static-assets) example serves a bundled
`dist/` alongside a JSON API.

## Answering NATS subjects (optional)

With the `nats` feature, `#[nats_route]` answers core-NATS request-reply on a
subject the same way `#[route]` answers an HTTP path — same `Request`, same
`Response`, same `FlatbedRouteError`, same two wire formats:

```rust
#[nats_route("plonk.ground.report.worldstate", queue = "ground")]
async fn ingest(req: Request<WorldStateDigest, Arc<Ctx>>)
    -> Result<Response<Ack>, FlatbedRouteError>
{
    req.ctx.store(&req.body).await?;
    Ok(Response::ok(Ack { accepted: true }))
}

// A {token} segment subscribes as a NATS wildcard and reaches the handler
// as a named param.
#[nats_route("plonk.satellite.{id}.call.status")]
async fn status(req: Request<StatusQuery, Arc<Ctx>>)
    -> Result<Response<SatelliteStatus>, FlatbedRouteError>
{
    Ok(Response::ok(req.ctx.status_of(req.param("id").unwrap_or_default()).await?))
}
```

Each responder is discovered through `inventory` (declaring the module is
enough) and runs as a worker that subscribes on the context's client, so the
context type must implement `HasNatsClient`. A `queue` group makes one replica
answer each request, which is how these scale horizontally; without one, every
replica answers every request.

**Every request is answered.** The request's `Content-Type` picks the encoding
for both directions (FlatBuffers when absent). A handler error comes back as a
reply carrying `x-error-code`, `x-error-message`, and `x-error-status` — as do
an undecodable payload and a panicking handler — so a requester's timeout never
means its request was rejected, only that the subject was unreachable or the
handler never returned.

## Asking on NATS subjects (optional)

`typed_request` is the other side of that contract: it encodes a body, waits
for the reply, and decodes it into the type the call site binds.

```rust
use flatbed::NatsRequestExt;

let status: SatelliteStatus = ctx.nats
    .typed_request("plonk.satellite.x07.call.status", &StatusQuery::default())
    .timeout(Duration::from_secs(2))
    .await?;
```

FlatBuffers unless `.encoding(NatsEncoding::Json)` says otherwise, in both
directions; five seconds unless `.timeout(...)` says otherwise. A handler's
rejection comes back as `NatsRequestError::Reply` carrying the
`FlatbedRouteError` the handler returned, so the responder's status and code
survive the hop — and `?` inside an HTTP handler propagates it, mapping an
unreachable subject to `502` and a silent one to `504`. Nothing subscribed is
`NoResponders`, distinct from the `Timeout` a responder that never answers
produces.

## Holding the NATS connection (optional)

`Connector` owns the connection lifecycle a boot function would otherwise
hand-roll, and reports the connection's state into readiness.

```rust
Flatbed::run(config, |cfg| async move {
    let nats = flatbed::nats::Connector::new("nats://broker:4222")
        .credentials_file("/etc/nats/user.creds")
        .readiness(cfg.readiness.gate("nats"))
        .connect_with_retry()
        .await?;

    Ok(AppContext { nats })
})
.await
```

The first connect is retried with a capped, jittered backoff under a bounded
budget, so a broker that has not started yet is waited out while a
misconfigured address still fails the boot instead of hanging forever. A
credentials file is read at connect time, so a missing secret mount is a
connect error rather than a panic at startup.

Readiness is two things: the one-shot boot latch, and any number of *gates*.
A gate is a named dependency that can come and go for the life of the process
— `/readyz` returns 200 only when the boot latch is set and every gate is
ready, and names the blocking gates in its 503 body. Declared routes answer
503 for the same interval, the way they already do during boot, so readiness
stays one notion rather than two that can disagree.

The connector opens the gate it is given while connected and closes it while
disconnected, draining, or closed, so a dropped broker connection takes the
pod out of its Service endpoints for exactly the interval the connection is
down. Reconnection is unbounded: a broker that is away for an hour is waited
out rather than giving up on the pod. Nothing registers a gate on your behalf
— a service with no gates behaves exactly as before.

## Crates

| Crate | Purpose |
|---|---|
| [`flatbed`](crates/flatbed) | HTTP server, route registry, optional telemetry / OpenAPI / NATS / Kubernetes feature gates |
| [`flatbed_macros`](crates/flatbed_macros) | `#[route]`, `#[nats_route]`, `#[worker]`, and `#[flatbed::main]` procedural macros |
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
