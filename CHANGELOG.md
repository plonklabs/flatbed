# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project adheres to semver — except that `0.0.x` releases are treated
as breaking by Cargo's compatibility rules, so every `0.0.x` bump may
contain breaking changes during the pre-1.0 stabilization window.

## [Unreleased]

### Added

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

[Unreleased]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.2...HEAD
[0.0.2]: https://github.com/plonklabs/flatbed/compare/flatbed-v0.0.1...flatbed-v0.0.2
[0.0.1]: https://github.com/plonklabs/flatbed/releases/tag/flatbed-v0.0.1
