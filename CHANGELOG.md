# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project adheres to semver — except that `0.0.x` releases are treated
as breaking by Cargo's compatibility rules, so every `0.0.x` bump may
contain breaking changes during the pre-1.0 stabilization window.

## [Unreleased]

## [0.0.8] — 2026-10-25

### Added

- `ErrorReporter` trait and `FlatbedConfig::with_error_reporter`: server
  failures are reported to an injected backend instead of a hard-wired one.
  Route failures answering 5xx over HTTP or NATS (`route_error`), caught
  panics in route handlers and workers with a backtrace (`panic`), and
  worker failures — typed `Err`, `Ok(())` exit, restarts exhausted
  (`worker_failure`) — are reported, never with headers, query strings or
  bodies. A flatbed panic hook is installed only when a reporter is
  configured and chains to the previous hook. `flush(2s)` runs when
  shutdown starts, before the drains. Log lines accompanying a report carry
  `flatbed.reported`.
- `sentry` feature: `flatbed::sentry::SentryReporter` (`from_env`, `layer`,
  `with_logs`), an `ErrorReporter` for Sentry and Sentry-protocol servers
  (GlitchTip, Bugsink). Uses the `ureq` + rustls (ring) transport, so no
  OpenSSL or `ca-certificates` is needed, and sends nothing without a DSN.
  Includes `examples/sentry` and a README section.

### Changed

- **BREAKING:** `#[flatbed::main]` and the hidden `flatbed::tokio` re-export
  are removed. Call `Flatbed::run` from your own `#[tokio::main]`.
- **BREAKING:** the minimum supported Rust version is now 1.88.
- **BREAKING:** an HTTP handler that panics is now caught and answered with
  `500` code `PANIC` and the request's `x-request-id`; before, the panic
  dropped the connection.
- HTTP 5xx answers are now logged (`route failed`).

### Fixed

- The k8s controllers no longer log each reconcile failure twice.

## [0.0.7] — 2026-10-21

### Added

- Cursor pagination. A table marked `(paginated)` in the schema is a list
  request (`page: string`, `page_size: uint`, every other field a typed
  filter) or, when it carries a `Page` field (`next_page`, `previous_page`),
  a list response. Codegen implements the new `flatbed::PageRequest` and
  `flatbed::PageResponse<R>` traits for them, and fails the build with a
  message naming the table when the shape is wrong. List routes are `POST`.
- `pagination` feature (new optional dependencies `hmac`, `sha2`, `base64`):
  `Pager::page` turns a request and a fetch closure over `ListQuery` into a
  `PageResponse`, with `Direction::Forward` / `Backward` paging and a
  default/maximum `page_size`. Cursors are opaque, HMAC-signed, and bound to
  the route and caller; a bad cursor answers `400 INVALID_CURSOR`, and a
  cursor combined with any filter answers `400 CURSOR_WITH_FILTERS`.
- `@plonklabs/flatbed-client` emits paginated list calls for `(paginated)`
  operations.

## [0.0.6] — 2026-10-21

### Added

- Codegen supports FlatBuffer unions of tables. A union field becomes
  `Option<Union>`, a Rust enum with one tuple variant per member, encoded with
  flatc's own discriminator + value layout. In JSON it is flatc's two sibling
  keys, `"<field>_type": "<Variant>"` and `"<field>": { … }`; an absent union
  writes neither, and `"NONE"` reads as absent.
- Codegen supports `(bit_flags)` enums. The field is flatc's `bitflags` type;
  in JSON it is the list of set flag names in declaration order (`[]` for
  none). An unknown name fails deserialization; set bits the schema doesn't
  name are kept by the FlatBuffer codec and fail JSON serialization.
- OpenAPI: a `bit_flags` enum component is an array of its flag names; a union
  component is a `oneOf` of its member tables, and a union field adds the
  `<field>_type` string-enum discriminator beside it. `flatbed::UnionSchema`
  registers unions; `EnumSchema` gains `bit_flags: bool`.

### Changed

- `validate_type_registry` reports a bare name shared across namespaces by
  any two of a table, enum, or union, since they share one OpenAPI component
  namespace. `TypeConflict.kind` is replaced by `first_kind` and
  `second_kind`.
- Table fields named after a Rust keyword (`ref`, `type`, …) generate flatc's
  escaped identifier (`ref_`) and keep the schema name on the JSON wire.
- `@plonklabs/flatbed-client` and `flatbed gen-fb-plugin` emit unions as
  `{ type, value }` and `bit_flags` fields as lists of flag names, in both the
  FlatBuffer and JSON codecs; 64-bit `bit_flags` enums are accepted.

## [0.0.5] — 2026-10-17

### Added

- `Connector::inbox_prefix`: replaces the `_INBOX` prefix of the subjects the
  client subscribes to for request replies, JetStream API calls, and publish
  acks, for broker accounts whose subscribe permissions are scoped to a
  namespace.
- `Connector::attempt_timeout`: bounds a single connect attempt inside the
  connect deadline, so a dial that hangs on a listener that is not up yet is
  abandoned and retried.

## [0.0.4] — 2026-09-12

### Added

- A route answers in the codec the request's `accept` header names
  (`application/json` or `application/x-flatbuffers`); the body is still
  decoded by `content-type`, and an `accept` naming neither leaves the
  response following `content-type` as before. `flatbed::accepted_codec` and
  `flatbed::Codec` are the rule, shared by the route wrapper and the
  `before_request` error path.
- `static_route!(..., no_fallback = ["/api/"])`: mount-relative path prefixes
  under which a miss is a 404 and never the SPA fallback. `StaticRouteInfo` gains
  `no_fallback: &'static [&'static str]`.

## [0.0.3] — 2026-09-12

### Added

- `static_route!(mount = "/", embed = "web/dist", fallback = "index.html")`:
  a static mount over a directory compiled into the binary, for a binary that
  has no image to ship `dist/` in. `embed` is relative to the crate's manifest
  and is exclusive with `dir`; the fallback, content types and cache headers
  are the filesystem mount's. `StaticRouteInfo.dir` is now
  `StaticRouteInfo.source: StaticSource`, `Dir(&str)` or
  `Embedded(&include_dir::Dir)`, and `flatbed` re-exports `include_dir`.
- `flatbed --version` on the `flatbed_build` CLI binary, reporting the
  installed crate version — previously an unrecognized-argument error.
- `#[nats_route]` (feature `nats`): core-NATS request-reply responders, the
  subject-transport sibling of `#[route]`. Handlers take the same
  `Request<T, Arc<C>>` and return the same `Result<Response<U>,
  FlatbedRouteError>`; each is registered through `inventory` and runs as a
  worker that subscribes on the context's client (`HasNatsClient`). Subjects
  can capture `{token}` segments as request params, an optional `queue` group
  spreads requests across replicas, and every request carrying a reply subject
  is answered — a handler error, an undecodable payload, and a panicking
  handler all come back as error replies carrying `x-error-code`,
  `x-error-message`, and `x-error-status`, so a requester's timeout never means
  its request was rejected. Overlapping subjects are rejected at startup, and a
  responder whose subscription ends fails its worker rather than leaving the
  process healthy and the subject silent.
- `Readiness` / `ReadinessGate`: runtime readiness alongside the one-shot boot
  latch. A gate is a named dependency that can come and go after boot; the
  server reports ready only when the boot latch is set and every registered
  gate is. `FlatbedConfig::readiness` carries the registry, so a boot function
  can register a gate from the config it already receives, and `/readyz` names
  the gates holding it down instead of a bare `Not Ready`.
- `flatbed::nats::Connector` (feature `nats`): a managed core-NATS connection.
  Retries the first connect with a capped, jittered backoff under a hard time
  budget, loads credentials inline or from a file read at connect time,
  jitters the client's own reconnect delays, and drives a `ReadinessGate` from
  the connection's state — so a broker that drops takes `/readyz` to 503 for
  the interval it is down, and restores it when the client reconnects. A link
  that closes its socket is noticed at once; a silent partition is noticed
  after a few ping intervals, for which the connector shortens the client's
  default. Reconnection is unbounded; a long broker outage is waited out
  rather than giving up on the process.

### Changed

- A `503` from a user route now distinguishes a boot that has not finished
  (`BOOTING`) from a readiness gate reporting its dependency unusable
  (`NOT_READY`, naming the gates). Readiness now covers the whole HTTP
  surface for gates as it already did for the boot latch: a closed gate 503s
  declared routes and static files, not only `/readyz`.
- **Breaking:** `FlatbedConfig` gains a public `readiness` field, so struct
  literals that named every field no longer compile. Builders are unaffected.
- **Breaking:** `ServiceContext::ready_rx` is renamed `booted_rx`, and
  `ServiceContext::is_ready` now means booted *and* every gate ready. The
  old one-shot meaning is available as the new `is_booted`.

## [0.0.2] — 2026-07-21

### Added

- `static_route!` macro: mount a directory of static files (with SPA
  history-fallback) alongside declared routes, registered through the same
  `inventory` mechanism as `#[route]`.
- `Response::raw(bytes, content_type)`: a raw-bytes response escape hatch that
  emits bytes verbatim under any `Content-Type`, bypassing JSON/FlatBuffer
  serialization (the basis for static serving and any HTML/CSV/image handler).
- Type-complete OpenAPI: a runtime type registry (`inventory`) records every
  generated table and enum — not just route I/O types — and `build_json_schema`
  is now a recursive `$ref`-based builder. Nested-only tables become resolvable
  components, enums and arrays render correctly, integer widths carry a
  `format`, and each property carries `x-fbs-*` vendor extensions so the exact
  wire layout is recoverable from the spec alone.
- Declared FlatBuffer field defaults surface as serde defaults on the generated
  structs, so an omitted field round-trips through its schema default.
- `flatbed gen-fb-plugin` CLI subcommand: pulls a served OpenAPI spec (or reads
  one from a file), reflects local `.fbs` schemas, validates they agree, and
  generates a zero-dependency TypeScript FlatBuffer client — `types.ts`
  (interfaces + numeric enums), `codec.ts` (per-table encode/decode over the
  `flatbuffers` runtime, byte-verified against the Rust codec), and `client.ts`
  (a `fetch` client with one method per route that talks
  `application/x-flatbuffers`).
- `examples/` folder with runnable flatbed services, including static-asset and
  raw-response examples.

### Changed

- **Breaking:** the telemetry config now reads `FLATBED_SERVICE_NAME`,
  `FLATBED_SERVICE_ADDRESS`, and `FLATBED_TELEMETRY_PORT` environment
  variables, renamed from their `PLONK_*` predecessors. Update any
  deployment that set the old names.

## [0.0.1] — 2026-06-26

### Added

- Initial extraction from `plonklabs/plonk` as a standalone repository
  on crates.io. Carries the framework's existing surface: the `#[route]`
  / `#[worker]` macros, the FlatBuffer codegen helper, the Hyper-backed
  server, and the optional `openapi` / `telemetry` / `nats` / `k8s`
  feature gates.

[Unreleased]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.8...HEAD
[0.0.8]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.7...flatbed-v0.0.8
[0.0.7]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.6...flatbed-v0.0.7
[0.0.6]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.5...flatbed-v0.0.6
[0.0.5]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.4...flatbed-v0.0.5
[0.0.4]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.3...flatbed-v0.0.4
[0.0.3]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.2...flatbed-v0.0.3
[0.0.2]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.1...flatbed-v0.0.2
[0.0.1]: https://github.com/plonklabs/flatbed/releases/tag/flatbed-v0.0.1
