# openapi

A flatbed service built with the `openapi` feature. The `#[route]` macro
captures each route's request/response types, `tag`, `summary`, and `version` at
compile time, and flatbed serves a generated OpenAPI 3 document — no hand-kept
spec.

Endpoints:

- `POST /greet` — tag `Greetings`
- `POST /echo` — tag `Utility`
- `GET /openapi.json` — spec for the latest version
- `GET /v1/openapi.json` — a specific version

## Run

```bash
docker compose up --build
```

This also starts a [Swagger UI](https://swagger.io/tools/swagger-ui/) container
pointed at the service's spec.

Locally (needs the pinned `flatc` on your `PATH`):

```bash
cargo run
```

## Try it

```bash
curl -s localhost:8080/greet -H 'content-type: application/json' -d '{"name":"Jose"}'
# => {"greeting":"Hello, Jose!"}

curl -s localhost:8080/echo  -H 'content-type: application/json' -d '{"message":"hi","times":3}'
# => {"message":"hi hi hi"}

curl -s localhost:8080/openapi.json | jq .
```

With `docker compose up`, open **http://localhost:8081** for the Swagger UI and
explore / call the endpoints from the browser.

> Point Redoc or a client generator at `/openapi.json` and it stays in sync with
> the code automatically.
