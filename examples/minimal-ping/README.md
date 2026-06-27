# minimal-ping

The smallest possible flatbed service: a single `POST /ping` route, no
application context, no optional features beyond what codegen requires.

It demonstrates the core flatbed promise — **one handler serves two wire
formats**. flatbed picks the codec from the request's `Content-Type` and
mirrors it on the response:

| Request `Content-Type`      | Body parsed as    | Response encoded as |
| --------------------------- | ----------------- | ------------------- |
| `application/json`          | JSON              | JSON                |
| `application/x-flatbuffers` | binary FlatBuffer | binary FlatBuffer   |

## Run

```bash
docker compose up --build
```

Or locally (needs the pinned `flatc` — see the version in `/.flatc-version` —
on your `PATH`):

```bash
cargo run
```

## Try it

```bash
# JSON — easy to hit from curl or a browser during development
curl -s localhost:8080/ping \
  -H 'content-type: application/json' \
  -d '{"message":"hi"}'
# => {"message":"pong: hi","success":true}

# Splash page
curl -s localhost:8080/
```

Service-to-service callers send `Content-Type: application/x-flatbuffers` with a
packed FlatBuffer body and get FlatBuffer bytes back — the same handler, no code
change.

## Notes

- The `openapi` feature is enabled in `Cargo.toml` even though this example
  doesn't serve a spec: flatbed's codegen always emits utoipa's `ToSchema`
  derive, which only resolves with that feature on. See the
  [`openapi`](../openapi) example to actually serve the generated spec.
- `flatbuffers` and `serde` are direct dependencies because the generated
  wire-format code references those crates by name.
