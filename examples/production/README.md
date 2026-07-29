# production

A flatbed service with every production feature switched on — the shape you'd
actually deploy as a box and generate a typed client against. Where the other
examples isolate one capability, this one wires them together on one listener:

- `POST /greet` — the business route
- `GET /openapi.json` — generated OpenAPI 3 spec (`openapi` feature)
- `GET /schema.bfbs` — FlatBuffer reflection, so a client can codegen from the
  running server
- `GET /healthz`, `/readyz`, `/metrics` — telemetry (`prometheus` feature). The
  readiness probe is what lets a Kubernetes/Plonk deployment start receiving
  traffic; without it the pod never goes Ready.
- a `Dockerfile`, so it ships as a container

`/metrics` reports a `greets_total` counter bumped on every `/greet`.

## Run

Locally (needs the pinned `flatc` on your `PATH`):

```bash
cargo run

curl -s localhost:8080/greet -H 'content-type: application/json' -d '{"name":"Jose"}'
# => {"greeting":"Hello, Jose!"}
```

As a container (from the repo root, so the path dependency resolves):

```bash
docker compose -f examples/production/docker-compose.yml up --build
```

## Generate a typed client

Point [`@plonklabs/flatbed-client`](../../clients/ts/flatbed-client) at the
running server — it reads `/openapi.json` + `/schema.bfbs` and emits a typed
client:

```bash
npx @plonklabs/flatbed-client generate --server http://localhost:8080 --out src/api
```
