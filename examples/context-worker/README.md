# context-worker

Shows two things flatbed services almost always need: an **application context**
and a **background worker**.

- The boot closure passed to `Flatbed::run` builds an `AppContext` once. The
  framework stores it *before* signaling ready, so every request handler and
  every worker observes the same value (no startup race).
- The `POST /info` handler reads the context via `Request<T, Arc<AppContext>>`.
- A `Worker` (`Heartbeat`) runs in the background — spawned only after boot
  completes — and logs the shared context to prove it has access.

> Note the context type in a handler is `Arc<AppContext>`, not `AppContext`: the
> framework wraps your context in an `Arc` and the `#[route]` macro requires that
> form. Workers receive `Arc<AppContext>` too.

## Run

```bash
docker compose up --build
```

Locally (needs the pinned `flatc` on your `PATH`):

```bash
cargo run
```

## Try it

```bash
curl -s localhost:8080/info -H 'content-type: application/json' -d '{"name":"Jose"}'
# => {"greeting":"Hello, Jose!","started_at":"boot-12345"}
```

Watch the worker in the logs — `started_at` matches the value the handler
returns, confirming they share one context:

```bash
docker compose logs -f app
# [heartbeat] alive — context started_at=boot-12345
```

## Wiring a worker

A worker is any `Default` type implementing the `Worker` trait, registered with
`register_worker!`:

```rust
impl Worker for Heartbeat {
    type Context = AppContext;
    const NAME: &'static str = "heartbeat";
    fn run(&self, ctx: Arc<Self::Context>) -> BoxFuture<Result<(), FlatbedWorkerError>> { /* … */ }
}
flatbed::register_worker!(Heartbeat, AppContext);
```

Registration is compile-time (via `inventory`) — declaring it is enough, no
manual wiring into `main`. If a worker returns an error after startup, flatbed
fails the liveness probe so the orchestrator restarts the pod.
