# raw-response

A handler's typed return value is serialized to JSON or FlatBuffer. When you
need a body that path can't express — CSV, SVG, an image, a rendered page —
`Response::raw` emits bytes verbatim under any `Content-Type`.

```rust
#[route("/report.csv", method = "POST")]
async fn report_csv(req: Request<CsvRequest>) -> Result<Response<()>, FlatbedRouteError> {
    let csv = build_csv(&req.body);
    Ok(Response::raw(csv.into_bytes(), "text/csv; charset=utf-8"))
}
```

`Response::raw(bytes, content_type)` returns `Response<()>`; the `#[route]`
wrapper forwards the bytes and content-type as-is instead of serializing.

## Run

Locally (needs `flatc` on `PATH`, matching `.flatc-version`):

```bash
cargo run

# A CSV report, built at request time:
curl -s -X POST localhost:8080/report.csv \
  -H 'content-type: application/json' -d '{"label":"widget","count":3}' -D -
# → content-type: text/csv; charset=utf-8
#   index,label
#   1,widget
#   2,widget
#   3,widget

# The same primitive, a different media type:
curl -s -X POST localhost:8080/badge.svg \
  -H 'content-type: application/json' -d '{"label":"ok"}' -D -
# → content-type: image/svg+xml
#   <svg ...>ok</svg>
```

Or with Docker:

```bash
docker compose up --build
```

For serving *static* files (a bundled SPA) rather than dynamically-generated
bodies, see [`static-assets`](../static-assets).
