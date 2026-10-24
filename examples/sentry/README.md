# sentry

A flatbed service built with the `sentry` feature. `SentryReporter::from_env()`
sends every 5xx answer and caught panic to the server in `SENTRY_DSN` — Sentry,
GlitchTip, or Bugsink — and its tracing layer turns the service's logs into
breadcrumbs. Without `SENTRY_DSN` the reporter does nothing.

Endpoints:

- `GET /fail` — logs a warning, then answers `500 UPSTREAM_DOWN`
- `GET /panic` — indexes past the end of a `Vec`; flatbed answers `500 PANIC`

## Run against a local Bugsink

```bash
docker compose up -d bugsink
```

Open **http://localhost:8000**, log in as `admin@example.com` / `admin`, create
a team and a project, and copy the project's DSN
(`http://<key>@localhost:8000/<id>`). Then start the app — inside compose the
Bugsink host is `bugsink`, not `localhost`:

```bash
SENTRY_DSN=http://<key>@bugsink:8000/<id> docker compose up --build app
```

Or run it locally (needs the pinned `flatc` on your `PATH`):

```bash
SENTRY_DSN=http://<key>@localhost:8000/<id> SENTRY_ENVIRONMENT=local cargo run
```

## Try it

```bash
curl -s localhost:8080/fail    # {"code":"UPSTREAM_DOWN","message":"inventory service unavailable"}
curl -s localhost:8080/panic   # {"code":"PANIC","message":"handler panicked"}
```

Bugsink shows two issues:

- `UPSTREAM_DOWN: inventory service unavailable`, tagged `route=/fail`,
  `method=GET`, `transport=http`, `status=500`, `code=UPSTREAM_DOWN`, and the
  response's `request_id`, with the `inventory service timed out` warning as a
  breadcrumb;
- `panic: index out of bounds: ...`, with the backtrace as its stacktrace
  pointing at `src/main.rs`.

flatbed's own `route failed` log line is not a third issue: it carries
`flatbed.reported = true`, so the layer keeps it as a breadcrumb. Stopping the
app (`Ctrl-C` or `SIGTERM`) flushes anything still queued.
