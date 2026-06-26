# flatbed

FlatBuffers utilities and route decorator system for the flatbed framework.

## Overview

`flatbed` provides two main features:

1. **FlatBuffer Utilities**: Helper functions for working with FlatBuffers
2. **Route Decorator System**: Declarative routing with compile-time validation

## Network Architecture

flatbed is designed to run behind a proxy that terminates TLS — a reverse
proxy, API gateway, or service-mesh sidecar (Envoy, nginx, Linkerd, …):

- **No TLS support** - the proxy handles TLS termination, mTLS, and external routing
- **HTTP/1.1 and HTTP/2 cleartext** - Both protocols auto-negotiated
- Traffic between the proxy and the service is plaintext, which keeps the
  framework small

## Features

### FlatBuffer Utilities

Helper functions for creating and verifying FlatBuffer messages:

```rust
use flatbed::{new_builder, verify_buffer, get_root};
use my_types::PingRequest;

// Create a builder
let mut builder = new_builder();

// Verify a buffer
let is_valid = verify_buffer::<PingRequest>(&bytes);

// Safely read a root with verification
let request = get_root::<PingRequest>(&bytes)?;
```

### Route Decorator System

Declarative route registration with automatic handler wrapping and compile-time validation.

#### Basic Usage

```rust
use flatbed::{route, Request, Response, FlatbedError};
use my_types::{PingRequest, PingResponse};

#[route("/ping")]
async fn handle_ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedError> {
    let msg = req.body.message.as_deref().unwrap_or("no message");

    Ok(Response::ok(PingResponse {
        message: Some(format!("pong: {}", msg)),
        timestamp: req.body.timestamp,
        success: true,
    }))
}
```

#### HTTP Methods

By default, routes are POST. Use the `method` attribute for other HTTP methods:

```rust
// GET endpoint
#[route("/users/{id}", method = "GET", tag = "Users")]
async fn get_user(req: Request<GetUserRequest>) -> Result<Response<UserResponse>, FlatbedError> {
    let id = req.param("id").unwrap();
    // ...
}

// PUT endpoint
#[route("/users/{id}", method = "PUT", tag = "Users")]
async fn update_user(req: Request<UpdateUserRequest>) -> Result<Response<UserResponse>, FlatbedError> {
    // ...
}

// POST endpoint (explicit)
#[route("/users", method = "POST", tag = "Users")]
async fn create_user(req: Request<CreateUserRequest>) -> Result<Response<UserResponse>, FlatbedError> {
    Ok(Response::created(user))
}
```

#### Handler Signature

Handlers must follow this pattern:
- **Async**: Must be `async fn`
- **Input**: `Request<T>` or `Request<T, C>` where `T` is the request body type and `C` is optional context
- **Output**: `Result<Response<T>, FlatbedError>` or `Result<Response<T>, FlatbedError<D>>` for typed error details

#### Request Type

`Request<T, C>` provides access to HTTP metadata:

```rust
pub struct Request<T, C = ()> {
    pub body: T,              // Deserialized request body
    pub ctx: C,               // Application context
    pub headers: HeaderMap,   // HTTP headers
    pub method: Method,       // HTTP method
    pub path: String,         // Request path
    pub path_params: HashMap<String, String>,   // Path parameters
    pub query_params: HashMap<String, String>,  // Query parameters
    pub request_id: String,   // X-Request-ID (auto-generated if missing)
}

impl<T, C> Request<T, C> {
    fn header(&self, name: &str) -> Option<&str>;  // Get header value
    fn param(&self, name: &str) -> Option<&str>;   // Get path parameter
    fn query(&self, name: &str) -> Option<&str>;   // Get query parameter
}
```

#### Response Type

`Response<T>` provides builder methods for HTTP responses:

```rust
// 200 OK
Ok(Response::ok(body))

// 201 Created
Ok(Response::created(body))

// Custom status
Ok(Response::with_status(body, StatusCode::ACCEPTED))

// With custom headers
Ok(Response::ok(body)
    .header("x-custom", "value")
    .header("cache-control", "max-age=3600"))
```

#### Error Handling

`FlatbedError` provides structured error responses:

```rust
// Simple error
Err(FlatbedError::bad_request("Invalid input"))

// Error with code
Err(FlatbedError::not_found("User not found")
    .code("USER_NOT_FOUND"))

// Error with custom headers
Err(FlatbedError::unauthorized("Token expired")
    .code("TOKEN_EXPIRED")
    .header("www-authenticate", "Bearer"))

// Error with typed details
Err(FlatbedError::bad_request("Validation failed")
    .code("VALIDATION_ERROR")
    .with_details(ValidationDetails {
        field: Some("email".into()),
        reason: Some("Invalid format".into()),
    }))
```

**Error Response Format:**

For JSON requests:
```json
{
  "code": "VALIDATION_ERROR",
  "message": "Validation failed",
  "details": { "field": "email", "reason": "Invalid format" }
}
```

For FlatBuffer requests, errors use headers:
- `Content-Type: application/x-flatbuffers`
- `X-Error-Code: VALIDATION_ERROR`
- `X-Error-Message: Validation failed`
- Body: FlatBuffer-serialized details (if present)

#### Context Support

Handlers can access application context (database connections, services, etc.):

```rust
struct AppCtx {
    db: DatabasePool,
}

#[route("/users")]
async fn create_user(req: Request<CreateUserRequest, AppCtx>) -> Result<Response<UserResponse>, FlatbedError> {
    let user = req.ctx.db.create_user(&req.body).await?;
    Ok(Response::created(UserResponse { name: user.name, ... }))
}
```

#### Route Registry

Access all registered routes at runtime:

```rust
use flatbed::{get_routes, validate_routes};

// Validate that all routes are unique
match validate_routes() {
    Ok(()) => println!("All routes valid!"),
    Err(conflict) => eprintln!("Route conflict: {}", conflict),
}

// Get the route map
let routes = get_routes();

for (path, route_info) in routes.iter() {
    println!("Route: {} {} ({} -> {})",
        route_info.method,
        path,
        route_info.request_type,
        route_info.response_type
    );
}
```

### Server Integration

Use `Flatbed::run()` to start the HTTP server with a boot closure:

```rust
use flatbed::{Flatbed, FlatbedConfig};

struct AppContext {
    db: DatabasePool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("My API")
        .host("0.0.0.0")
        .port(8080)
        .description("My awesome API");

    Flatbed::run(config, |config| async move {
        println!("Listening on {}:{}", config.host, config.port);

        let db = DatabasePool::connect().await?;
        Ok(AppContext { db })
    }).await?;

    Ok(())
}
```

The boot closure receives a clone of the `FlatbedConfig` and returns the application
context. The framework handles all probe signaling automatically — callers never
touch health or readiness channels directly.

#### Boot Lifecycle

`Flatbed::run()` follows a strict sequence that ensures Kubernetes probes behave
correctly at every stage, and that workers never start before the application
context is available:

```
Flatbed::run(config, boot)
  │
  ├─ 1. Validate routes (fail-fast on conflicts)
  ├─ 2. Bind TCP listener
  ├─ 3. Spawn HTTP server task
  ├─ 4. Set healthz = true          ──► /healthz → 200 (pod is alive)
  │                                      /readyz  → 503 (not ready)
  │                                      routes   → 503 (blocked)
  ├─ 5. Run boot(config) ───────────── user init (connect DB, kube client, etc.)
  │     └─ returns AppContext
  ├─ 6. Store context
  ├─ 7. Set ready = true            ──► /readyz  → 200 (accepting traffic)
  │                                      routes   → 200 (serving)
  └─ 8. Spawn workers               ──► workers have guaranteed access to context
```

**Why this order matters:**

- **healthz before boot** — Kubernetes won't kill the pod during slow initialization
  (e.g., waiting for a database connection). The pod is alive, just not ready yet.
- **Context stored before ready** — Workers are spawned only after the ready signal.
  By storing context first, workers are guaranteed to find it when they start.
  This eliminates the race condition where a worker could wake up and find no context.
- **Workers after ready** — Background tasks (reconcilers, heartbeats) only run once
  the server is fully initialized.

**Worker failure handling:** If any worker returns an error after startup, the
framework sets `healthz = false` and initiates graceful shutdown. Kubernetes will
see the failing liveness probe and restart the pod.

#### With Telemetry

```rust
use flatbed::{Flatbed, FlatbedConfig};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize telemetry BEFORE server starts
    let telemetry = PrometheusTelemetryService::new(tel_config);

    // Register metrics during startup
    let counter = telemetry.register_u64_counter("requests_total", "Total requests", None)?;
    counter.inc();

    let config = FlatbedConfig::new("My API")
        .host("0.0.0.0")
        .port(8080)
        .with_telemetry(telemetry);

    Flatbed::run(config, |config| async move {
        println!("Server starting on {}:{}", config.host, config.port);
        let db = DatabasePool::connect().await?;
        Ok(AppContext { db })
    }).await?;

    Ok(())
}
```

#### Legacy: `#[flatbed::main]` Macro (Deprecated)

> **Note**: The `#[flatbed::main]` macro is deprecated. Use `Flatbed::run()` instead for more flexibility.

```rust
use flatbed::main;

#[flatbed::main(bind = "0.0.0.0:8080")]
async fn main() {
    println!("Server started");
}
```

The `Flatbed::run()` API provides:
- Pre-server setup flexibility (telemetry, logging, diagnostics)
- Content-type negotiation (JSON and FlatBuffer)
- X-Request-ID propagation (or auto-generation)
- Graceful shutdown on SIGTERM/SIGINT
- HTTP/1.1 and HTTP/2 cleartext support (auto-negotiated)
- Automatic route validation at startup

#### Built-in Endpoints

When features are enabled, the server provides built-in endpoints:

**Telemetry** (`telemetry` feature):
- `GET /healthz` - Liveness probe (returns "OK")
- `GET /readyz` - Readiness probe (returns "Ready")
- `GET /metrics` - Prometheus metrics

**OpenAPI** (`openapi` feature):
- `GET /openapi.json` - OpenAPI specification (latest version)
- `GET /v{version}/openapi.json` - Versioned OpenAPI specification

## How It Works

The route decorator system uses procedural macros and the [`inventory`](https://crates.io/crates/inventory) crate to collect route registrations across your codebase at compile time.

When you write:

```rust
#[route("/ping")]
async fn handle_ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedError> { ... }
```

The macro generates:

1. **Wrapper function** - Extracts HTTP metadata, deserializes body, calls your handler, serializes response
2. **Route registration** - Submits route info to the global registry via `inventory`

The registry is built at compile time and available via `get_routes()` at runtime.

**Note**: Routes are collected per-binary. Test binaries, examples, and your main application each have their own independent route registry.

## Compile-Time Validation

Route conflicts are detected via `validate_routes()`:

```rust
// This will return an error if two routes have the same path
// but different request/response types
match validate_routes() {
    Ok(()) => {/* All good */},
    Err(conflict) => panic!("Duplicate route: {}", conflict.path),
}
```

Always call `validate_routes()` at startup to catch configuration errors early. `Flatbed::run()` does this automatically.

## Examples

See `examples/routes.rs` for route registration examples:

```bash
cargo run --example routes -p flatbed
```

## Testing

Run the test suite:

```bash
cargo test -p flatbed
```

Run with all features:

```bash
cargo test -p flatbed --features "openapi,telemetry"
```

## Architecture

- **flatbed** (this crate): FlatBuffer utilities + hyper server + route registry
- **flatbed_macros**: Procedural macro implementation for `#[route]` and `#[flatbed::main]`
- **flatbed_build**: Build-time code generation for FlatBuffer schemas. See its crate docs for the supported `.fbs` field types (scalars, strings, single nested tables, and vectors of any of those — including vectors of tables).

The macro crate is separate because Rust requires `proc-macro` crates to only export procedural macros.

## Dependencies

- `flatbuffers` - FlatBuffer runtime
- `inventory` - Distributed plugin registration
- `http` - HTTP types (Method, StatusCode, HeaderMap)
- `hyper` / `hyper-util` - HTTP server (no TLS - the fronting proxy handles it)
- `tokio` - Async runtime
- `serde` / `serde_json` - JSON serialization
- `flatbed_macros` - Route decorator and main macros

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option. See the [repository root](../../README.md#license) for details.
