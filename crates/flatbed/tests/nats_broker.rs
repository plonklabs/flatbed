//! Broker-backed integration tests for the JetStream worker layer:
//! [`flatbed::nats::StreamWorker`] and [`flatbed::kv::KvWorker`] running
//! against a real NATS server with JetStream enabled.
//!
//! The tests are marked `#[ignore]` so the plain workspace test run stays
//! broker-free. Stream and bucket names are fixed, so two checkouts aimed
//! at one broker clobber each other's state; `scripts/nats-broker.sh`
//! starts a broker scoped to the current worktree and prints its address:
//!
//! ```text
//! scripts/nats-broker.sh up
//! NATS_URL=$(scripts/nats-broker.sh url) \
//!   cargo test -p flatbed --features nats,openapi --test nats_broker -- --ignored
//! ```
//!
//! `NATS_URL` defaults to `localhost:4222`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_nats::jetstream;
use flatbed::kv::{run_kv_worker, KvWorker};
use flatbed::nats::{run_stream_worker, HasJetStream, NatsResult, StreamWorker};
use flatbed::BoxFuture;
use generated::test::{LogEvent, Severity};

// ============================================================================
// Test infrastructure
// ============================================================================

/// Everything a worker observed, recorded through its context so tests can
/// assert on exact handler invocations.
#[derive(Debug, Clone, PartialEq)]
enum Handled {
    Msg(u32),
    Event(LogEvent),
    Put { key: String, n: u32 },
    Delete { key: String },
}

struct TestCtx {
    jetstream: jetstream::Context,
    handled: Mutex<Vec<Handled>>,
    #[cfg(feature = "k8s")]
    leader_rx: tokio::sync::watch::Receiver<bool>,
}

impl TestCtx {
    fn record(&self, event: Handled) {
        self.handled.lock().unwrap().push(event);
    }

    fn snapshot(&self) -> Vec<Handled> {
        self.handled.lock().unwrap().clone()
    }

    fn count(&self, event: &Handled) -> usize {
        self.handled
            .lock()
            .unwrap()
            .iter()
            .filter(|h| *h == event)
            .count()
    }
}

impl HasJetStream for TestCtx {
    fn jetstream(&self) -> &jetstream::Context {
        &self.jetstream
    }
}

// With the k8s feature compiled in, `run_stream_worker` consults
// `ha_enabled()` before consuming; returning `false` keeps the broker tests
// on the standalone consume path regardless of feature unification.
#[cfg(feature = "k8s")]
impl flatbed::k8s::HasLeaderElection for TestCtx {
    fn is_leader_rx(&self) -> tokio::sync::watch::Receiver<bool> {
        self.leader_rx.clone()
    }

    fn ha_enabled(&self) -> bool {
        false
    }
}

async fn test_ctx() -> Arc<TestCtx> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "localhost:4222".to_string());
    let client = async_nats::connect(&url)
        .await
        .unwrap_or_else(|e| panic!("cannot reach NATS at {url} (see module docs): {e}"));

    #[cfg(feature = "k8s")]
    let leader_rx = tokio::sync::watch::channel(false).1;

    Arc::new(TestCtx {
        jetstream: jetstream::new(client),
        handled: Mutex::new(Vec::new()),
        #[cfg(feature = "k8s")]
        leader_rx,
    })
}

/// Delete-then-create so re-runs against a long-lived local broker never
/// inherit stream, durable-consumer, or message state from a prior run.
async fn recreate_stream(ctx: &TestCtx, name: &str, subject: &str) -> jetstream::stream::Stream {
    ctx.jetstream.delete_stream(name).await.ok();
    ctx.jetstream
        .create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .expect("stream create failed")
}

/// KV buckets are JetStream streams named `KV_<bucket>` on the wire, so the
/// reset goes through `delete_stream` (works on every async-nats version).
async fn recreate_bucket(ctx: &TestCtx, bucket: &str) -> jetstream::kv::Store {
    ctx.jetstream
        .delete_stream(format!("KV_{bucket}"))
        .await
        .ok();
    ctx.jetstream
        .create_key_value(jetstream::kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
        .expect("bucket create failed")
}

async fn publish(ctx: &TestCtx, subject: &str, payload: Vec<u8>) {
    ctx.jetstream
        .publish(subject.to_string(), payload.into())
        .await
        .expect("publish failed")
        .await
        .expect("publish unacked by broker");
}

fn spawn_stream_worker<W>(ctx: &Arc<TestCtx>) -> tokio::task::JoinHandle<()>
where
    W: StreamWorker<Context = TestCtx> + Default,
{
    let any: Arc<dyn Any + Send + Sync> = ctx.clone();
    tokio::spawn(async move {
        run_stream_worker::<W, TestCtx>(any)
            .await
            .expect("stream worker exited with error");
    })
}

fn spawn_kv_worker<W>(ctx: &Arc<TestCtx>) -> tokio::task::JoinHandle<()>
where
    W: KvWorker<Context = TestCtx> + Default,
{
    let any: Arc<dyn Any + Send + Sync> = ctx.clone();
    tokio::spawn(async move {
        run_kv_worker::<W, TestCtx>(any)
            .await
            .expect("kv worker exited with error");
    })
}

const DEADLINE: Duration = Duration::from_secs(20);

async fn wait_for(what: &str, deadline: Duration, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out after {deadline:?} waiting for {what}");
}

/// Wait until the durable consumer has no pending or in-flight messages —
/// the broker-side proof that every delivered message was ACKed.
async fn wait_consumer_drained(stream: &jetstream::stream::Stream, durable: &str) {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < DEADLINE {
        if let Ok(info) = stream.consumer_info(durable).await {
            if info.num_ack_pending == 0 && info.num_pending == 0 {
                return;
            }
            last = format!(
                "num_ack_pending={} num_pending={}",
                info.num_ack_pending, info.num_pending
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("consumer '{durable}' never drained: {last}");
}

fn u32_payload(n: u32) -> Vec<u8> {
    n.to_le_bytes().to_vec()
}

fn parse_u32(bytes: &[u8]) -> Result<u32, String> {
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_| format!("expected 4 bytes, got {}", bytes.len()))?;
    Ok(u32::from_le_bytes(arr))
}

// ============================================================================
// Publish → durable consume → ACK (through the register macro's glue)
// ============================================================================

#[derive(Default)]
struct AckWorker;

impl StreamWorker for AckWorker {
    type Message = u32;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-ack-worker";
    const STREAM: &'static str = "FLATBED_IT_ACK";
    const SUBJECT: &'static str = "flatbed.it.ack";

    fn handle(&self, ctx: Arc<TestCtx>, msg: u32) -> BoxFuture<NatsResult> {
        Box::pin(async move {
            ctx.record(Handled::Msg(msg));
            NatsResult::Ack
        })
    }

    fn parse_message(bytes: &[u8]) -> Result<u32, String> {
        parse_u32(bytes)
    }
}

flatbed::register_stream_worker!(AckWorker, TestCtx);

/// Publishes through JetStream, consumes through the durable pull consumer,
/// and verifies broker-side ACK state. The worker is driven through the
/// `WorkerInfo` entry that `register_stream_worker!` submitted, so the
/// macro-generated glue is part of the path under test.
#[tokio::test]
#[ignore]
async fn stream_worker_consumes_and_acks_published_messages() {
    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, AckWorker::STREAM, AckWorker::SUBJECT).await;

    let info = flatbed::get_workers()
        .into_iter()
        .find(|w| w.name == AckWorker::NAME)
        .expect("register_stream_worker! must make the worker discoverable");
    let any: Arc<dyn Any + Send + Sync> = ctx.clone();
    let task = tokio::spawn((info.worker)(any));

    for n in [1u32, 2, 3] {
        publish(&ctx, AckWorker::SUBJECT, u32_payload(n)).await;
    }

    wait_for("3 handled messages", DEADLINE, || ctx.snapshot().len() == 3).await;
    assert_eq!(
        ctx.snapshot(),
        vec![Handled::Msg(1), Handled::Msg(2), Handled::Msg(3)],
        "messages must arrive in publish order, decoded",
    );

    wait_consumer_drained(&stream, AckWorker::NAME).await;
    task.abort();
}

// ============================================================================
// Retry: an unACKed message is redelivered
// ============================================================================

#[derive(Default)]
struct RetryWorker;

impl StreamWorker for RetryWorker {
    type Message = u32;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-retry-worker";
    const STREAM: &'static str = "FLATBED_IT_RETRY";
    const SUBJECT: &'static str = "flatbed.it.retry";

    fn handle(&self, ctx: Arc<TestCtx>, msg: u32) -> BoxFuture<NatsResult> {
        Box::pin(async move {
            let prior = ctx.count(&Handled::Msg(msg));
            ctx.record(Handled::Msg(msg));
            if prior == 0 {
                NatsResult::Retry("first delivery fails".to_string())
            } else {
                NatsResult::Ack
            }
        })
    }

    fn parse_message(bytes: &[u8]) -> Result<u32, String> {
        parse_u32(bytes)
    }
}

/// `NatsResult::Retry` leaves the message unACKed; the broker redelivers it
/// once the consumer's ack_wait expires (server default 30s — the framework
/// exposes no ack_wait or NAK control, so redelivery latency is bound to
/// that default; hence this test's long deadline).
#[tokio::test]
#[ignore]
async fn stream_worker_retry_leads_to_redelivery_then_ack() {
    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, RetryWorker::STREAM, RetryWorker::SUBJECT).await;
    let task = spawn_stream_worker::<RetryWorker>(&ctx);

    publish(&ctx, RetryWorker::SUBJECT, u32_payload(7)).await;

    wait_for(
        "redelivery of the retried message",
        Duration::from_secs(75),
        || ctx.count(&Handled::Msg(7)) >= 2,
    )
    .await;

    wait_consumer_drained(&stream, RetryWorker::NAME).await;
    task.abort();
}

// ============================================================================
// Skip: ACKed without being treated as processed
// ============================================================================

#[derive(Default)]
struct SkipWorker;

impl StreamWorker for SkipWorker {
    type Message = u32;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-skip-worker";
    const STREAM: &'static str = "FLATBED_IT_SKIP";
    const SUBJECT: &'static str = "flatbed.it.skip";

    fn handle(&self, ctx: Arc<TestCtx>, msg: u32) -> BoxFuture<NatsResult> {
        Box::pin(async move {
            if msg % 2 == 1 {
                return NatsResult::Skip(format!("odd payload {msg}"));
            }
            ctx.record(Handled::Msg(msg));
            NatsResult::Ack
        })
    }

    fn parse_message(bytes: &[u8]) -> Result<u32, String> {
        parse_u32(bytes)
    }
}

/// A skipped message is ACKed (no redelivery) but the handler treated it as
/// not-processed. The drained consumer plus the absence of a second delivery
/// within the observation window is the skip contract.
#[tokio::test]
#[ignore]
async fn stream_worker_skip_acks_without_redelivery() {
    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, SkipWorker::STREAM, SkipWorker::SUBJECT).await;
    let task = spawn_stream_worker::<SkipWorker>(&ctx);

    publish(&ctx, SkipWorker::SUBJECT, u32_payload(1)).await; // skipped
    publish(&ctx, SkipWorker::SUBJECT, u32_payload(2)).await; // processed

    wait_for("the processed message", DEADLINE, || {
        ctx.count(&Handled::Msg(2)) == 1
    })
    .await;
    wait_consumer_drained(&stream, SkipWorker::NAME).await;

    assert_eq!(
        ctx.snapshot(),
        vec![Handled::Msg(2)],
        "the skipped message must not be recorded as processed",
    );
    task.abort();
}

// ============================================================================
// Parse failure: ACK-to-skip keeps the consumer moving
// ============================================================================

/// An undecodable payload is ACKed to prevent redelivery, the handler never
/// runs for it, and the consumer keeps processing subsequent messages.
#[tokio::test]
#[ignore]
async fn stream_worker_acks_undecodable_payload_and_continues() {
    #[derive(Default)]
    struct StrictWorker;

    impl StreamWorker for StrictWorker {
        type Message = u32;
        type Context = TestCtx;
        type ParseError = String;

        const NAME: &'static str = "it-strict-worker";
        const STREAM: &'static str = "FLATBED_IT_STRICT";
        const SUBJECT: &'static str = "flatbed.it.strict";

        fn handle(&self, ctx: Arc<TestCtx>, msg: u32) -> BoxFuture<NatsResult> {
            Box::pin(async move {
                ctx.record(Handled::Msg(msg));
                NatsResult::Ack
            })
        }

        fn parse_message(bytes: &[u8]) -> Result<u32, String> {
            parse_u32(bytes)
        }
    }

    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, StrictWorker::STREAM, StrictWorker::SUBJECT).await;
    let task = spawn_stream_worker::<StrictWorker>(&ctx);

    publish(&ctx, StrictWorker::SUBJECT, vec![0xFF, 0xFE]).await; // undecodable
    publish(&ctx, StrictWorker::SUBJECT, u32_payload(9)).await;

    wait_for("the decodable message", DEADLINE, || {
        ctx.count(&Handled::Msg(9)) == 1
    })
    .await;
    wait_consumer_drained(&stream, StrictWorker::NAME).await;

    assert_eq!(
        ctx.snapshot(),
        vec![Handled::Msg(9)],
        "the undecodable payload must never reach the handler",
    );
    task.abort();
}

// ============================================================================
// FlatBuffers round-trip through the broker
// ============================================================================

#[derive(Default)]
struct FbWorker;

impl StreamWorker for FbWorker {
    type Message = LogEvent;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-fb-worker";
    const STREAM: &'static str = "FLATBED_IT_FB";
    const SUBJECT: &'static str = "flatbed.it.fb";

    fn handle(&self, ctx: Arc<TestCtx>, msg: LogEvent) -> BoxFuture<NatsResult> {
        Box::pin(async move {
            ctx.record(Handled::Event(msg));
            NatsResult::Ack
        })
    }

    fn parse_message(bytes: &[u8]) -> Result<LogEvent, String> {
        LogEvent::from_flatbuffer(bytes).map_err(|e| e.to_string())
    }
}

/// A FlatBuffers payload published to the stream decodes back to the
/// original typed value in the handler. Non-default enum variants are pinned
/// so a decoder that silently falls back to defaults fails the assertion.
#[tokio::test]
#[ignore]
async fn stream_worker_round_trips_flatbuffers_payload() {
    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, FbWorker::STREAM, FbWorker::SUBJECT).await;
    let task = spawn_stream_worker::<FbWorker>(&ctx);

    let original = LogEvent {
        message: Some("disk full".to_string()),
        severity: Severity::Error,
        history: Some(vec![Severity::Info, Severity::Warning, Severity::Error]),
    };
    publish(&ctx, FbWorker::SUBJECT, original.to_flatbuffer()).await;

    wait_for("the decoded event", DEADLINE, || !ctx.snapshot().is_empty()).await;
    assert_eq!(ctx.snapshot(), vec![Handled::Event(original)]);

    wait_consumer_drained(&stream, FbWorker::NAME).await;
    task.abort();
}

// ============================================================================
// Durable consumer survives a worker restart
// ============================================================================

#[derive(Default)]
struct DurableWorker;

impl StreamWorker for DurableWorker {
    type Message = u32;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-durable-worker";
    const STREAM: &'static str = "FLATBED_IT_DURABLE";
    const SUBJECT: &'static str = "flatbed.it.durable";

    fn handle(&self, ctx: Arc<TestCtx>, msg: u32) -> BoxFuture<NatsResult> {
        Box::pin(async move {
            ctx.record(Handled::Msg(msg));
            NatsResult::Ack
        })
    }

    fn parse_message(bytes: &[u8]) -> Result<u32, String> {
        parse_u32(bytes)
    }
}

/// The durable consumer's cursor lives on the broker: a restarted worker
/// resumes after the last ACKed message instead of reprocessing history.
#[tokio::test]
#[ignore]
async fn stream_worker_durable_consumer_resumes_after_restart() {
    let ctx = test_ctx().await;
    let stream = recreate_stream(&ctx, DurableWorker::STREAM, DurableWorker::SUBJECT).await;

    let first = spawn_stream_worker::<DurableWorker>(&ctx);
    publish(&ctx, DurableWorker::SUBJECT, u32_payload(11)).await;
    wait_for("first message handled", DEADLINE, || {
        ctx.count(&Handled::Msg(11)) == 1
    })
    .await;
    wait_consumer_drained(&stream, DurableWorker::NAME).await;
    first.abort();

    publish(&ctx, DurableWorker::SUBJECT, u32_payload(22)).await;
    publish(&ctx, DurableWorker::SUBJECT, u32_payload(33)).await;

    let ctx2 = Arc::new(TestCtx {
        jetstream: ctx.jetstream.clone(),
        handled: Mutex::new(Vec::new()),
        #[cfg(feature = "k8s")]
        leader_rx: ctx.leader_rx.clone(),
    });
    let second = spawn_stream_worker::<DurableWorker>(&ctx2);

    wait_for("messages published while down", DEADLINE, || {
        ctx2.snapshot().len() >= 2
    })
    .await;
    assert_eq!(
        ctx2.snapshot(),
        vec![Handled::Msg(22), Handled::Msg(33)],
        "the restarted worker must resume after the ACKed message, not replay it",
    );
    second.abort();
}

// ============================================================================
// KV: boot replay, live updates, delete/purge fan-in
// ============================================================================

#[derive(Default)]
struct WatchWorker;

impl KvWorker for WatchWorker {
    type Value = u32;
    type Context = TestCtx;
    type ParseError = String;

    const NAME: &'static str = "it-kv-watch-worker";
    const BUCKET: &'static str = "FLATBED_IT_KV";

    fn on_put(&self, ctx: Arc<TestCtx>, key: String, value: u32) -> BoxFuture<()> {
        Box::pin(async move { ctx.record(Handled::Put { key, n: value }) })
    }

    fn on_delete(&self, ctx: Arc<TestCtx>, key: String) -> BoxFuture<()> {
        Box::pin(async move { ctx.record(Handled::Delete { key }) })
    }

    fn parse_value(bytes: &[u8]) -> Result<u32, String> {
        parse_u32(bytes)
    }
}

flatbed::register_kv_worker!(WatchWorker, TestCtx);

fn put_event(key: &str, n: u32) -> Handled {
    Handled::Put {
        key: key.to_string(),
        n,
    }
}

fn delete_event(key: &str) -> Handled {
    Handled::Delete {
        key: key.to_string(),
    }
}

/// Puts a fresh canary revision every 200ms until the worker observes one.
/// This is the only way to know the worker's watch is open: `watch_all()`
/// under async-nats 0.38 delivers only entries written after the watch was
/// established (deliver-policy New), and the executor exposes no
/// watch-established signal.
async fn wait_watch_open(ctx: &TestCtx, store: &jetstream::kv::Store) {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        store
            .put("canary", u32_payload(0).into())
            .await
            .expect("canary put failed");
        tokio::time::sleep(Duration::from_millis(200)).await;
        if ctx.count(&put_event("canary", 0)) > 0 {
            return;
        }
    }
    panic!("kv watch never delivered the canary");
}

/// A running KV worker receives live puts, updates, deletes, and purges —
/// delete and purge both arriving as `on_delete`. Entries written before
/// the watch opened are never delivered (`watch_all()` is deliver-policy
/// New in async-nats 0.38), so there is no boot replay of existing state;
/// the seeded key pins that. Driven through the `register_kv_worker!`
/// `WorkerInfo` entry so the macro glue is part of the path under test.
#[tokio::test]
#[ignore]
async fn kv_worker_follows_put_update_delete_purge() {
    let ctx = test_ctx().await;
    let store = recreate_bucket(&ctx, WatchWorker::BUCKET).await;

    store
        .put("pre", u32_payload(1).into())
        .await
        .expect("seed put failed");

    let info = flatbed::get_workers()
        .into_iter()
        .find(|w| w.name == WatchWorker::NAME)
        .expect("register_kv_worker! must make the worker discoverable");
    let any: Arc<dyn Any + Send + Sync> = ctx.clone();
    let task = tokio::spawn((info.worker)(any));

    wait_watch_open(&ctx, &store).await;

    store
        .put("beta", u32_payload(2).into())
        .await
        .expect("put failed");
    wait_for("live put", DEADLINE, || {
        ctx.count(&put_event("beta", 2)) == 1
    })
    .await;

    store
        .put("beta", u32_payload(3).into())
        .await
        .expect("update failed");
    wait_for("live update", DEADLINE, || {
        ctx.count(&put_event("beta", 3)) == 1
    })
    .await;

    store.delete("beta").await.expect("delete failed");
    wait_for("delete event", DEADLINE, || {
        ctx.count(&delete_event("beta")) == 1
    })
    .await;

    store
        .put("gamma", u32_payload(4).into())
        .await
        .expect("put failed");
    store.purge("gamma").await.expect("purge failed");
    wait_for("purge as delete event", DEADLINE, || {
        ctx.count(&delete_event("gamma")) == 1
    })
    .await;

    assert_eq!(
        ctx.count(&put_event("pre", 1)),
        0,
        "an entry written before the watch opened must not be delivered — \
         watch_all() performs no boot replay",
    );
    task.abort();
}

/// One undecodable value is logged and skipped; the watch keeps delivering
/// subsequent entries instead of tearing down.
#[tokio::test]
#[ignore]
async fn kv_worker_skips_undecodable_value_and_keeps_watching() {
    #[derive(Default)]
    struct StrictKvWorker;

    impl KvWorker for StrictKvWorker {
        type Value = u32;
        type Context = TestCtx;
        type ParseError = String;

        const NAME: &'static str = "it-kv-strict-worker";
        const BUCKET: &'static str = "FLATBED_IT_KV_STRICT";

        fn on_put(&self, ctx: Arc<TestCtx>, key: String, value: u32) -> BoxFuture<()> {
            Box::pin(async move { ctx.record(Handled::Put { key, n: value }) })
        }

        fn on_delete(&self, ctx: Arc<TestCtx>, key: String) -> BoxFuture<()> {
            Box::pin(async move { ctx.record(Handled::Delete { key }) })
        }

        fn parse_value(bytes: &[u8]) -> Result<u32, String> {
            parse_u32(bytes)
        }
    }

    let ctx = test_ctx().await;
    let store = recreate_bucket(&ctx, StrictKvWorker::BUCKET).await;
    let task = spawn_kv_worker::<StrictKvWorker>(&ctx);

    wait_watch_open(&ctx, &store).await;

    store
        .put("bad", vec![0xAB].into())
        .await
        .expect("put failed");
    store
        .put("good", u32_payload(5).into())
        .await
        .expect("put failed");

    wait_for("the decodable entry", DEADLINE, || {
        ctx.count(&put_event("good", 5)) == 1
    })
    .await;

    assert!(
        !ctx.snapshot()
            .iter()
            .any(|h| matches!(h, Handled::Put { key, .. } if key == "bad")),
        "the undecodable value must be skipped without stopping the watch",
    );
    task.abort();
}
