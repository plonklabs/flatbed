# openapi

A flatbed service built with the `openapi` feature. The `#[route]` macro
captures each route's request/response types, `tag`, `summary`, and `version` at
compile time, and flatbed serves a generated OpenAPI 3 document — no hand-kept
spec.

The schema exercises the full type surface: a `Priority` enum, a nested-only
`Tag` table (reached only by nesting inside `EchoRequest`, never a route body),
and a `[Tag]` vector. The generated spec renders these as a string `enum`, a
component referenced by `$ref`, and an array — each carrying `x-fbs-type` /
`x-fbs-id` extensions — so it's a complete, `$ref`-resolvable contract.

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

## Verify a TypeScript client generates and type-checks

`scripts/verify-openapi-ts.sh` (from the repo root) boots this service, pulls
`/openapi.json`, generates TypeScript with
[`openapi-typescript`](https://openapi-ts.dev), and type-checks it with `tsc`:

```bash
bash scripts/verify-openapi-ts.sh
```

A green run is proof that a JSON client generated from the spec compiles — the
enum becomes a `"Low" | "Medium" | "High"` union, `Tag` its own interface, and
`tags` a `Tag[]`. It needs `node`/`npx` and skips cleanly when they're absent.

## Generate a FlatBuffer binary client

The JSON path above works with any OpenAPI tool. For a client that talks the
FlatBuffer wire format directly, `flatbed gen-fb-plugin` cross-checks the served
spec against the local `.fbs` and emits a self-contained TypeScript codec:

```bash
flatbed gen-fb-plugin --server http://localhost:8080 \
                      --schemas-dir schemas/ --out ./fb-client
```

It writes three files: `types.ts` (interfaces + numeric enums), `codec.ts`
(per-table `encode…Root` / `decode…Root` over the zero-dependency
[`flatbuffers`](https://www.npmjs.com/package/flatbuffers) runtime — `npm i
flatbuffers`), and `client.ts` (a `fetch` client with one method per route):

```ts
const client = new FlatbedClient({ baseUrl: "http://localhost:8080" });
const res = await client.postEcho({ message: "hi", times: 3, priority: Priority.Low });
// => { message: "hi hi hi" }  — sent and received as application/x-flatbuffers
```

Two scripts prove it end to end: `scripts/verify-fb-codec.sh` round-trips every
value through the Rust and TS codecs to show they agree byte-for-byte, and
`scripts/verify-fb-client.sh` boots this service and has the generated client
call it over `application/x-flatbuffers`.
