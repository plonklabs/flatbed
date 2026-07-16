# spa

A flatbed service that serves a JSON API **and** a bundled single-page app from
one origin. The frontend and its API share a host, so there's no CORS and no
separate static-server box.

- `POST /api/hello` — a normal `#[route]` handler returning JSON.
- Everything else (GET) — served from `dist/` by `static_route!`, with unknown
  non-API paths falling back to `index.html` for client-side routing.

```rust
#[route("/api/hello", method = "POST")]
async fn hello(req: Request<HelloRequest>) -> Result<Response<HelloResponse>, FlatbedRouteError> { ... }

// Declared routes win; unknown GETs are served from dist/.
static_route!(mount = "/", dir = "dist", fallback = "index.html");
```

`dir` is read from the container filesystem at request time and resolved
relative to the process working directory. The `Dockerfile` sets `WORKDIR /app`
and `COPY examples/spa/dist ./dist`, so `dir = "dist"` resolves to `/app/dist`.
In a real project `dist/` is your bundler's output (e.g. Vite's `npm run build`);
the checked-in `dist/` here is a hand-written stand-in.

## Run

Locally (needs `flatc` on `PATH`, matching `.flatc-version`):

```bash
cargo run                      # cwd is examples/spa, so dir = "dist" resolves
curl -s -X POST localhost:8080/api/hello \
  -H 'content-type: application/json' -d '{"name":"you"}'   # {"message":"hello, you"}
curl -s localhost:8080/                 # index.html
curl -s localhost:8080/assets/app-a1b2c3.js   # the JS bundle, content-type text/javascript
curl -s localhost:8080/dashboard        # unknown route → index.html (SPA fallback)
```

Or with Docker:

```bash
docker compose up --build
```

## Content types and caching

`static_route!` picks the `Content-Type` from the file extension and sets
`Cache-Control`: `no-cache` for `*.html` (it references hashed asset URLs) and
`public, max-age=31536000, immutable` for everything else (bundlers content-hash
asset filenames, so they're safe to cache forever).
