# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project adheres to semver — except that `0.0.x` releases are treated
as breaking by Cargo's compatibility rules, so every `0.0.x` bump may
contain breaking changes during the pre-1.0 stabilization window.

## [Unreleased]

### Added

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
