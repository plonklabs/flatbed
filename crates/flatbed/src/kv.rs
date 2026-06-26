//! Fan-out NATS JetStream KV consumer for trait-based cache workers.
//!
//! `KvWorker` is the every-pod counterpart to [`crate::nats::StreamWorker`].
//! Where `StreamWorker` is a JetStream **subject** pull consumer with
//! exactly-once-across-pods semantics and follower-only HA gating,
//! `KvWorker` watches a **KV bucket** with fan-out semantics:
//! every pod runs its own watch and applies every change to a local
//! cache, including the leader. That's what makes it suitable for
//! "single writer → many readers" patterns (the canonical case being
//! a long-lived gRPC subscription handler that needs a low-latency
//! local view of state someone else publishes).
//!
//! ## Subscription semantics
//!
//! `async-nats`'s `kv::Store::watch_all()` (0.38+) emits **every
//! current entry first** (a key-inventory replay), then live `Put` /
//! `Delete` / `Purge` events. That's how every-pod boot rehydration
//! works for free — a freshly-started pod sees the same state as one
//! that's been running for hours.
//!
//! ## Error handling
//!
//! Stream errors trigger a 1-second backoff and reopen. Bucket
//! lookup failures (`get_key_value`) retry on a 5-second backoff —
//! the operator's `main.rs` creates buckets before spawning workers,
//! so persistent lookup failure means something's broken (kv ACL,
//! NATS reachability) and the worker should keep trying. Decode
//! failures on a Put log + skip the entry; one bad payload must not
//! tear down the whole subscription.

use std::sync::Arc;

use futures::StreamExt;
use tracing::{info, warn};

use crate::nats::HasJetStream;
use crate::{BoxFuture, FlatbedWorkerError};

// ============================================================================
// KvWorker Trait
// ============================================================================

/// Trait-based NATS JetStream KV cache worker.
///
/// Implement this trait to define a per-pod cache subscriber against
/// a KV bucket. The runtime executor [`run_kv_worker`] handles
/// bucket lookup, watch stream lifecycle, payload decoding, and
/// fail-soft reopen — matching the shape of [`crate::nats::StreamWorker`]
/// but without follower-only gating.
///
/// # Requirements
///
/// Implementors must also implement `Default` — the runtime executor
/// constructs the worker via `W::default()` once on startup. Add
/// `#[derive(Default)]` on the impl struct (or hand-roll a `Default`
/// impl if there's per-worker state to initialise).
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::kv::KvWorker;
/// use flatbed::BoxFuture;
/// use std::sync::Arc;
///
/// #[derive(Default)]
/// struct MyCacheSubscriber;
///
/// impl KvWorker for MyCacheSubscriber {
///     type Value = MySnapshot;
///     type Context = AppContext;
///
///     const NAME: &'static str = "my-cache-subscriber";
///     const BUCKET: &'static str = "MY_BUCKET";
///
///     fn on_put(&self, ctx: Arc<Self::Context>, key: String, value: Self::Value)
///         -> BoxFuture<()> {
///         Box::pin(async move { ctx.cache.apply(key, value) })
///     }
///
///     fn on_delete(&self, ctx: Arc<Self::Context>, key: String) -> BoxFuture<()> {
///         Box::pin(async move { ctx.cache.forget(&key) })
///     }
///
///     type ParseError = String;
///
///     fn parse_value(bytes: &[u8]) -> Result<Self::Value, Self::ParseError> {
///         MySnapshot::from_flatbuffer(bytes).map_err(|e| e.to_string())
///     }
/// }
///
/// flatbed::register_kv_worker!(MyCacheSubscriber, AppContext);
/// ```
///
/// `ParseError` is an associated type so workers can return a
/// typed enum (one variant per failure mode) instead of collapsing
/// every distinction into a `String`. The runtime logs via
/// `Display` and skips the entry, so any `Display + Send + 'static`
/// type satisfies the bound.
pub trait KvWorker: Send + Sync + 'static {
    /// Decoded value type stored under each key in the bucket.
    type Value: Send + 'static;

    /// The application context type providing JetStream access.
    type Context: HasJetStream + Send + Sync + 'static;

    /// Error type returned by [`Self::parse_value`]. The runtime
    /// logs failures via `Display` and skips the affected entry; no
    /// trait bound forces a particular shape, so workers can pick
    /// `String` for trivial single-mode decodes or a typed enum
    /// when failure modes need to be distinguishable in triage.
    type ParseError: std::fmt::Display + Send + 'static;

    /// Worker name used for logging.
    const NAME: &'static str;

    /// Optional description of what this worker does.
    const DESCRIPTION: Option<&'static str> = None;

    /// JetStream KV bucket name to watch.
    const BUCKET: &'static str;

    /// Handle a `Put` event: a key's value was set or changed.
    fn on_put(&self, ctx: Arc<Self::Context>, key: String, value: Self::Value) -> BoxFuture<()>;

    /// Handle a `Delete` or `Purge` event: a key was removed.
    fn on_delete(&self, ctx: Arc<Self::Context>, key: String) -> BoxFuture<()>;

    /// Deserialize raw bytes into the decoded value type. A failure
    /// here logs and skips the affected entry without tearing down
    /// the subscription.
    fn parse_value(bytes: &[u8]) -> Result<Self::Value, Self::ParseError>;
}

// ============================================================================
// Runtime Executor
// ============================================================================

/// Run a [`KvWorker`] forever.
///
/// Mirrors [`crate::nats::run_stream_worker`] in shape but with two
/// differences: (1) no follower-only gating — every pod (including
/// the leader) runs the watch, and (2) the source is a KV bucket
/// `watch_all()` stream, not a JetStream subject pull consumer.
pub async fn run_kv_worker<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    W: KvWorker<Context = C> + Default,
    C: HasJetStream + Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("kv_worker '{}' context type mismatch", W::NAME));

    let worker = W::default();

    loop {
        let store_result = {
            let jetstream = HasJetStream::jetstream(&*ctx);
            jetstream.get_key_value(W::BUCKET).await
        };

        let store = match store_result {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    worker = W::NAME,
                    bucket = W::BUCKET,
                    error = %e,
                    "KV bucket lookup failed; retrying in 5s",
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        watch_cycle::<W, C>(&worker, Arc::clone(&ctx), store).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Backoff applied inside [`watch_cycle`] before returning on a
/// watch failure (either `watch_all()` itself or a mid-stream entry
/// error). Without this, a persistent NATS-side failure (ACL,
/// broker partition) would loop the outer executor at roughly
/// 1 WARN/second, which dominates production log volume on a
/// misconfigured cluster.
const WATCH_FAILURE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// One open-watch-until-error cycle. Returning means the caller
/// should re-open the watch.
async fn watch_cycle<W, C>(worker: &W, ctx: Arc<C>, store: async_nats::jetstream::kv::Store)
where
    W: KvWorker<Context = C>,
    C: HasJetStream + Send + Sync + 'static,
{
    let mut watch = match store.watch_all().await {
        Ok(w) => w,
        Err(e) => {
            warn!(
                worker = W::NAME,
                bucket = W::BUCKET,
                error = %e,
                "KV watch_all failed; will reopen",
            );
            // Long backoff: this failure mode is usually persistent
            // (ACL, broker outage); the outer executor's post-cycle
            // 1 s sleep is too short to keep WARN volume sane.
            tokio::time::sleep(WATCH_FAILURE_BACKOFF).await;
            return;
        }
    };

    info!(worker = W::NAME, bucket = W::BUCKET, "subscribed");

    while let Some(entry_result) = watch.next().await {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    worker = W::NAME,
                    bucket = W::BUCKET,
                    error = %e,
                    "KV entry stream error; reopening",
                );
                // Same rationale as above: mid-stream errors are
                // often the same persistent failure mode as
                // watch_all() failing.
                tokio::time::sleep(WATCH_FAILURE_BACKOFF).await;
                return;
            }
        };

        let event = map_operation(entry.operation, entry.key, &entry.value);
        dispatch_event::<W, C>(worker, Arc::clone(&ctx), event).await;
    }

    // Stream ended cleanly after processing entries (e.g. NATS
    // server restart). Outer loop's short post-cycle sleep is fine
    // here — we want to reconnect quickly.
    info!(
        worker = W::NAME,
        bucket = W::BUCKET,
        "KV watch stream ended; reopening"
    );
}

/// Internal event shape decoupled from `async_nats::jetstream::kv::Entry`,
/// so [`dispatch_event`] and [`map_operation`] can be unit-tested
/// without constructing a real `Entry` (its fields are private) or a
/// `kv::Store`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KvEvent {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

/// Translate a raw `kv::Operation` into the testable [`KvEvent`]
/// shape. `Delete` and `Purge` collapse into a single
/// `KvEvent::Delete` because cache subscribers can't act on the
/// distinction — once a key is gone, it's gone. Lives apart from
/// [`watch_cycle`] so this collapse is unit-testable against the
/// real `kv::Operation` variants; without that, a refactor that
/// introduces a separate `KvEvent::Purge` would silently change
/// runtime behaviour while keeping the dispatch tests green.
///
/// The value reference is only copied into a `Vec` on the `Put`
/// branch — `Delete` / `Purge` events carry an empty payload from
/// NATS and don't need the allocation.
fn map_operation(op: async_nats::jetstream::kv::Operation, key: String, value: &[u8]) -> KvEvent {
    match op {
        async_nats::jetstream::kv::Operation::Put => KvEvent::Put {
            key,
            value: value.to_vec(),
        },
        async_nats::jetstream::kv::Operation::Delete
        | async_nats::jetstream::kv::Operation::Purge => KvEvent::Delete { key },
    }
}

/// Route a decoded KV event into the worker's `on_put` / `on_delete`
/// handlers. Lives apart from [`watch_cycle`] so the dispatch
/// invariants (parse-then-dispatch on `Put`, skip-on-decode-failure)
/// are testable without a NATS server. The `Delete` / `Purge`
/// collapse is pinned by [`map_operation`]'s tests instead.
async fn dispatch_event<W, C>(worker: &W, ctx: Arc<C>, event: KvEvent)
where
    W: KvWorker<Context = C>,
    C: HasJetStream + Send + Sync + 'static,
{
    match event {
        KvEvent::Put { key, value } => match W::parse_value(&value) {
            Ok(decoded) => {
                worker.on_put(ctx, key, decoded).await;
            }
            Err(e) => {
                warn!(
                    worker = W::NAME,
                    bucket = W::BUCKET,
                    key = %key,
                    error = %e,
                    "KV value decode failed; skipping entry",
                );
            }
        },
        KvEvent::Delete { key } => {
            worker.on_delete(ctx, key).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal value type for verifying `parse_value` behaviour.
    #[derive(Debug, PartialEq)]
    struct TestValue {
        n: u32,
    }

    /// Stub `HasJetStream` context for trait-shape tests. Never
    /// actually invoked — `parse_value` is a static method.
    struct TestCtx;

    impl HasJetStream for TestCtx {
        fn jetstream(&self) -> &async_nats::jetstream::Context {
            panic!("stub jetstream: not expected to be called in parse_value tests");
        }
    }

    struct TestKvWorker;

    impl Default for TestKvWorker {
        fn default() -> Self {
            Self
        }
    }

    impl KvWorker for TestKvWorker {
        type Value = TestValue;
        type Context = TestCtx;
        type ParseError = String;

        const NAME: &'static str = "test-kv-worker";
        const BUCKET: &'static str = "TEST_BUCKET";

        fn on_put(
            &self,
            _ctx: Arc<Self::Context>,
            _key: String,
            _value: Self::Value,
        ) -> BoxFuture<()> {
            Box::pin(async {})
        }

        fn on_delete(&self, _ctx: Arc<Self::Context>, _key: String) -> BoxFuture<()> {
            Box::pin(async {})
        }

        fn parse_value(bytes: &[u8]) -> Result<Self::Value, Self::ParseError> {
            if bytes.len() != 4 {
                return Err(format!("expected 4 bytes, got {}", bytes.len()));
            }
            let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(TestValue { n })
        }
    }

    /// Static method should round-trip a well-formed payload. Mirrors
    /// `StreamWorker::parse_message_valid_bytes` — same kind of static
    /// contract, same kind of tripwire.
    #[test]
    fn parse_value_valid_bytes() {
        let bytes = 42u32.to_le_bytes();
        let result = TestKvWorker::parse_value(&bytes);
        assert_eq!(result, Ok(TestValue { n: 42 }));
    }

    /// Malformed bytes must surface as `Err` rather than panic — the
    /// executor relies on this contract to log + skip without tearing
    /// down the watch.
    #[test]
    fn parse_value_invalid_bytes_returns_error() {
        let result = TestKvWorker::parse_value(&[1, 2]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 4 bytes"));
    }

    /// Empty payload is the degenerate case that's most likely to
    /// slip through an unwrap. Pin it.
    #[test]
    fn parse_value_empty_bytes_returns_error() {
        let result = TestKvWorker::parse_value(&[]);
        assert!(result.is_err());
    }

    /// Default `DESCRIPTION` is `None`. A regression that defaulted to
    /// `Some("")` would silently produce noisy worker registries.
    #[test]
    fn kv_worker_description_default_is_none() {
        assert!(<TestKvWorker as KvWorker>::DESCRIPTION.is_none());
    }

    /// Required constants match what the impl declares. Tripwire for
    /// a refactor that accidentally renames a trait constant.
    #[test]
    fn kv_worker_constants_match() {
        assert_eq!(<TestKvWorker as KvWorker>::NAME, "test-kv-worker");
        assert_eq!(<TestKvWorker as KvWorker>::BUCKET, "TEST_BUCKET");
    }

    // ========================================================================
    // Typed (non-String) ParseError
    // ========================================================================

    /// Pins that the `KvWorker` trait accepts a non-`String`
    /// `ParseError` so a regression that re-narrows the bound
    /// surfaces in flatbed's own test suite rather than only in any
    /// downstream consumer that depends on the wider bound.
    #[derive(Debug, PartialEq)]
    enum CustomKvParseError {
        WrongLen { got: usize, expected: usize },
    }

    impl std::fmt::Display for CustomKvParseError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::WrongLen { got, expected } => {
                    write!(f, "wrong length: got {got}, expected {expected}")
                }
            }
        }
    }

    struct TypedErrorKvWorker;

    impl KvWorker for TypedErrorKvWorker {
        type Value = TestValue;
        type Context = TestCtx;
        type ParseError = CustomKvParseError;

        const NAME: &'static str = "typed-error-kv-worker";
        const BUCKET: &'static str = "TYPED_BUCKET";

        fn on_put(
            &self,
            _ctx: Arc<Self::Context>,
            _key: String,
            _value: Self::Value,
        ) -> BoxFuture<()> {
            Box::pin(async {})
        }

        fn on_delete(&self, _ctx: Arc<Self::Context>, _key: String) -> BoxFuture<()> {
            Box::pin(async {})
        }

        fn parse_value(bytes: &[u8]) -> Result<Self::Value, Self::ParseError> {
            if bytes.len() != 4 {
                return Err(CustomKvParseError::WrongLen {
                    got: bytes.len(),
                    expected: 4,
                });
            }
            let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(TestValue { n })
        }
    }

    /// A typed `ParseError` round-trips through `parse_value` — the
    /// caller gets the structured variant, not a stringified form.
    #[test]
    fn typed_kv_parse_error_round_trips_structured_variant() {
        let err = TypedErrorKvWorker::parse_value(&[1, 2]).unwrap_err();
        assert_eq!(
            err,
            CustomKvParseError::WrongLen {
                got: 2,
                expected: 4
            }
        );
    }

    /// `Display` is what `dispatch_event` renders via
    /// `warn!(error = %e, …)`. Pin the formatted output so a refactor
    /// that changes the `Display` template surfaces here.
    #[test]
    fn typed_kv_parse_error_display_matches_expected_template() {
        let err = CustomKvParseError::WrongLen {
            got: 2,
            expected: 4,
        };
        assert_eq!(err.to_string(), "wrong length: got 2, expected 4");
    }

    // ========================================================================
    // dispatch_event tests — the routing core
    // ========================================================================

    use std::sync::Mutex;

    /// Recording worker so each test can assert exactly which
    /// handler fired and what it received. Fields are `Arc<Mutex<…>>`
    /// rather than plain `Mutex<…>` so the futures `on_put` /
    /// `on_delete` return are `'static` — `BoxFuture` requires it.
    #[derive(Default)]
    struct RecorderWorker {
        puts: Arc<Mutex<Vec<(String, TestValue)>>>,
        deletes: Arc<Mutex<Vec<String>>>,
    }

    impl KvWorker for RecorderWorker {
        type Value = TestValue;
        type Context = TestCtx;
        type ParseError = String;

        const NAME: &'static str = "recorder-worker";
        const BUCKET: &'static str = "RECORDER_BUCKET";

        fn on_put(
            &self,
            _ctx: Arc<Self::Context>,
            key: String,
            value: Self::Value,
        ) -> BoxFuture<()> {
            let puts = Arc::clone(&self.puts);
            Box::pin(async move {
                puts.lock().unwrap().push((key, value));
            })
        }

        fn on_delete(&self, _ctx: Arc<Self::Context>, key: String) -> BoxFuture<()> {
            let deletes = Arc::clone(&self.deletes);
            Box::pin(async move {
                deletes.lock().unwrap().push(key);
            })
        }

        fn parse_value(bytes: &[u8]) -> Result<Self::Value, Self::ParseError> {
            <TestKvWorker as KvWorker>::parse_value(bytes)
        }
    }

    /// `Put` with valid bytes → `parse_value` succeeds → `on_put`
    /// is called with the decoded value. This is the happy-path
    /// dispatch contract — the rest of the runtime executor depends
    /// on it.
    #[tokio::test]
    async fn dispatch_event_routes_put_to_on_put() {
        let worker = RecorderWorker::default();
        let ctx = Arc::new(TestCtx);

        dispatch_event::<RecorderWorker, TestCtx>(
            &worker,
            ctx,
            KvEvent::Put {
                key: "tenant-a/worker".into(),
                value: 42u32.to_le_bytes().to_vec(),
            },
        )
        .await;

        let puts = worker.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, "tenant-a/worker");
        assert_eq!(puts[0].1, TestValue { n: 42 });
        assert!(worker.deletes.lock().unwrap().is_empty());
    }

    /// `Delete` → `on_delete` fires; `on_put` does not. Pin the
    /// channel separation: a regression that wired delete events
    /// into the put path would silently treat tombstones as
    /// upserts.
    #[tokio::test]
    async fn dispatch_event_routes_delete_to_on_delete() {
        let worker = RecorderWorker::default();
        let ctx = Arc::new(TestCtx);

        dispatch_event::<RecorderWorker, TestCtx>(
            &worker,
            ctx,
            KvEvent::Delete {
                key: "tenant-a/worker".into(),
            },
        )
        .await;

        let deletes = worker.deletes.lock().unwrap();
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0], "tenant-a/worker");
        assert!(worker.puts.lock().unwrap().is_empty());
    }

    /// `Put` with bytes that fail `parse_value` → log+skip, no
    /// handler fires. The runtime relies on this contract to keep
    /// one bad payload from tearing down the watch.
    #[tokio::test]
    async fn dispatch_event_skips_put_when_decode_fails() {
        let worker = RecorderWorker::default();
        let ctx = Arc::new(TestCtx);

        dispatch_event::<RecorderWorker, TestCtx>(
            &worker,
            ctx,
            KvEvent::Put {
                key: "tenant-a/worker".into(),
                value: vec![0xFF, 0xFE], // not 4 bytes → parse error
            },
        )
        .await;

        assert!(
            worker.puts.lock().unwrap().is_empty(),
            "on_put must not fire when parse_value fails",
        );
        assert!(
            worker.deletes.lock().unwrap().is_empty(),
            "on_delete must not fire either — bad bytes are silently skipped",
        );
    }

    // ========================================================================
    // map_operation tests — the kv::Operation → KvEvent mapping
    // ========================================================================

    /// `Operation::Put` → `KvEvent::Put` carrying the payload bytes.
    /// Pin so a refactor that swapped the arms would surface here.
    #[test]
    fn map_operation_routes_put_to_put_event() {
        let event = map_operation(
            async_nats::jetstream::kv::Operation::Put,
            "tenant-a/worker".to_string(),
            &42u32.to_le_bytes(),
        );
        assert_eq!(
            event,
            KvEvent::Put {
                key: "tenant-a/worker".to_string(),
                value: 42u32.to_le_bytes().to_vec(),
            }
        );
    }

    /// `Operation::Delete` → `KvEvent::Delete`. Straightforward arm,
    /// but pinned so an accidental restructure can't silently route
    /// deletes through the put path.
    #[test]
    fn map_operation_routes_delete_to_delete_event() {
        let event = map_operation(
            async_nats::jetstream::kv::Operation::Delete,
            "tenant-a/worker".to_string(),
            b"",
        );
        assert_eq!(
            event,
            KvEvent::Delete {
                key: "tenant-a/worker".to_string(),
            }
        );
    }

    /// `Operation::Purge` → `KvEvent::Delete` — the collapse contract.
    /// This is the test the old `dispatch_event_treats_purge_as_delete`
    /// tried (and failed) to pin: it constructed `KvEvent::Delete`
    /// directly, so a refactor that introduced a separate
    /// `KvEvent::Purge` would still have passed. By calling
    /// `map_operation` with the real `kv::Operation::Purge` enum
    /// variant, this test would actually break under that refactor
    /// and force a deliberate decision.
    #[test]
    fn map_operation_routes_purge_to_delete_event() {
        let event = map_operation(
            async_nats::jetstream::kv::Operation::Purge,
            "tenant-a/worker".to_string(),
            b"",
        );
        assert_eq!(
            event,
            KvEvent::Delete {
                key: "tenant-a/worker".to_string(),
            }
        );
    }

    // ========================================================================
    // register_kv_worker! macro tests
    // ========================================================================

    crate::register_kv_worker!(TestKvWorker, TestCtx);

    /// The macro must submit a `WorkerInfo` entry that `inventory`
    /// surfaces via `get_workers()`. Without this round-trip, the
    /// `Flatbed::run` spawn loop would silently skip the worker.
    #[test]
    fn register_kv_worker_macro_makes_worker_discoverable() {
        let workers = crate::get_workers();
        let found = workers
            .iter()
            .find(|w| w.name == <TestKvWorker as KvWorker>::NAME);
        assert!(
            found.is_some(),
            "TestKvWorker should be discoverable via inventory"
        );
        let info = found.unwrap();
        assert_eq!(info.name, "test-kv-worker");
        assert_eq!(info.description, <TestKvWorker as KvWorker>::DESCRIPTION);
    }
}
