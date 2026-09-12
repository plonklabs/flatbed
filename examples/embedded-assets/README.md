# embedded-assets

A flatbed service that serves a JSON API **and** a bundled single-page app from
one origin, with the app compiled into the binary. The binary is the whole
deployable: an installed CLI, a single file copied to a host, a `scratch`
image — nothing is read from disk at request time.

- `POST /api/hello` — a normal `#[route]` handler returning JSON.
- Everything else (GET) — served from a copy of `dist/` taken at build time by
  `static_route!`, with unknown non-API paths falling back to `index.html` for
  client-side routing.

```rust
#[route("/api/hello", method = "POST")]
async fn hello(req: Request<HelloRequest>) -> Result<Response<HelloResponse>, FlatbedRouteError> { ... }

// Declared routes win; unknown GETs are served from the embedded dist/,
// except under /api/, where a miss is a 404.
static_route!(mount = "/", embed = "dist", fallback = "index.html", no_fallback = ["/api/"]);
```

`embed` is relative to this crate's `Cargo.toml` and is read once, at build
time; a change to `dist/` needs a rebuild. Content types, the `index.html`
fallback and the cache headers are the same as a `dir` mount's (see
[`static-assets`](../static-assets) for the filesystem form and the trade-off).
The `Dockerfile` copies only the binary into the runtime image — no `dist/`,
no `WORKDIR` — which is the point.

In a real project `dist/` is your bundler's output (e.g. Vite's `npm run build`)
and the build runs before `cargo build`; the checked-in `dist/` here is a
hand-written stand-in.

## Run

Locally (needs `flatc` on `PATH`, matching `.flatc-version`):

```bash
cargo run
curl -s -X POST localhost:8080/api/hello \
  -H 'content-type: application/json' -d '{"name":"you"}'   # {"message":"hello, you"}
curl -s localhost:8080/                 # index.html
curl -s localhost:8080/assets/app-a1b2c3.js   # the JS bundle, content-type text/javascript
curl -s localhost:8080/dashboard        # unknown route → index.html (SPA fallback)
curl -si localhost:8080/api/nothing | head -1   # HTTP/1.1 404: under no_fallback, not the shell
curl -s localhost:8080/api/hello -X POST -H 'accept: application/json' \
  -H 'content-type: application/x-flatbuffers' --data-binary @req.bin   # FlatBuffer in, JSON out
```

Or with Docker:

```bash
docker compose up --build
```
