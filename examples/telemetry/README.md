# telemetry

A flatbed service built with the `telemetry` + `prometheus` features. Enabling
telemetry auto-registers operational endpoints, and the app records a custom
counter that real traffic increments.

Endpoints:

- `POST /ping` — bumps a `ping_requests_total` counter
- `GET /healthz` — liveness probe (`OK`)
- `GET /readyz` — readiness probe (`Ready` once boot completes)
- `GET /metrics` — Prometheus text format

## Run

```bash
docker compose up --build
```

This also starts a Prometheus container that scrapes the service every 5s (see
`prometheus.yml`).

Locally (needs the pinned `flatc` on your `PATH`):

```bash
cargo run
```

## Try it

```bash
curl -s localhost:8080/healthz   # OK
curl -s localhost:8080/readyz    # Ready

curl -s localhost:8080/ping -H 'content-type: application/json' -d '{"message":"a"}'
curl -s localhost:8080/ping -H 'content-type: application/json' -d '{"message":"b"}'

curl -s localhost:8080/metrics | grep ping_requests_total
# ping_requests_total{ip_address="0.0.0.0",service="telemetry-example"} 2
```

With `docker compose up`, open the Prometheus UI at **http://localhost:9090** and
query `ping_requests_total`.

## Notes

- The counter is built from the `TelemetryService` in `main` and handed to the
  application context, so the `/ping` handler can `inc()` it. The `TelemetryConfig`
  attaches `service` / `ip_address` labels automatically.
- Health/readiness reflect the boot lifecycle: `/healthz` is `OK` as soon as the
  server binds; `/readyz` flips to `Ready` only after the boot closure returns
  the context.
