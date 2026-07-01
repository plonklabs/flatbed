# flatbed examples

Each subfolder is a **complete, standalone flatbed service** you can build, run,
and hit with `curl`. They are ordered from simplest to most featureful.

| Example | Demonstrates |
| ------- | ------------ |
| [`minimal-ping`](minimal-ping)       | The smallest service: one `#[route]`, and one handler serving **both** JSON and binary FlatBuffer. |
| [`openapi`](openapi)                 | The `openapi` feature — routes tagged at compile time, served as a generated OpenAPI 3 spec, with a Swagger UI. |
| [`telemetry`](telemetry)             | The `telemetry` + `prometheus` features — `/healthz`, `/readyz`, `/metrics`, a custom counter, and a Prometheus scraper. |
| [`context-worker`](context-worker)   | An application `AppContext` built in the boot closure plus a background `Worker`. |

## Running an example

Each example ships a `docker-compose.yml`. From inside the example's folder:

```bash
cd minimal-ping
docker compose up --build
```

The app listens on `localhost:8080`; see each example's README for the exact
endpoints and `curl` commands. Some examples start extra containers (Swagger UI
on `:8081`, Prometheus on `:9090`).

### Running without Docker

Every example is an ordinary Cargo project. To run one directly you need the
**pinned `flatc`** on your `PATH` — the version is in
[`/.flatc-version`](../.flatc-version) (currently `25.9.23`). flatbed's codegen
runs in each example's `build.rs`, so a normal `cargo run` regenerates the
FlatBuffer bindings:

```bash
cd minimal-ping
cargo run
```

Install the matching `flatc` from the
[FlatBuffers releases](https://github.com/google/flatbuffers/releases), or let
Docker handle it (each `Dockerfile` installs the pinned version).

## How these are structured

Each example is its own **standalone Cargo package** (note the empty
`[workspace]` table in its `Cargo.toml`), not a member of the flatbed workspace.
It depends on the framework by path (`../../crates/flatbed`), which is why every
`docker-compose.yml` sets the build `context` to the repo root.

A few constraints worth knowing, shared by all examples:

- **The `openapi` feature is always enabled**, even where a spec isn't served.
  flatbed's codegen unconditionally emits utoipa's `ToSchema` derive, which only
  resolves with that feature on.
- **`flatbuffers` and `serde` are direct dependencies**, because the generated
  wire-format code references those crates by name.
- **Generated code is never committed.** `build.rs` writes it into `OUT_DIR` and
  `main.rs` pulls it in with
  `include!(concat!(env!("OUT_DIR"), "/<schema>_flatbed.rs"))`.

## Not covered here

The `nats` (JetStream stream/KV workers) and `k8s` (reconcilers, watchers)
features need external infrastructure (a NATS server, a Kubernetes cluster) to
run, so they're left out of this folder for now. See the crate docs in
[`crates/flatbed`](../crates/flatbed) for those.
