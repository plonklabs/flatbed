//! Broker-backed integration tests for `#[nats_route]`: core-NATS
//! request-reply responders answering on a real NATS server.
//!
//! The tests are marked `#[ignore]` so the plain workspace test run stays
//! broker-free. `scripts/nats-broker.sh` starts a broker scoped to the
//! current worktree and prints its address:
//!
//! ```text
//! scripts/nats-broker.sh up
//! NATS_URL=$(scripts/nats-broker.sh url) \
//!   cargo test -p flatbed --features nats,openapi \
//!     --test nats_broker --test nats_route_broker -- --ignored
//! ```
//!
//! `NATS_URL` defaults to `localhost:4222`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_nats::{HeaderMap as NatsHeaderMap, Message};
use flatbed::nats::HasNatsClient;
use flatbed::nats_route::{
    ERROR_CODE_HEADER, ERROR_MESSAGE_HEADER, ERROR_STATUS_HEADER, REQUEST_ID_HEADER,
};
use flatbed::{nats_route, FlatbedRouteError, Request, Response};
use generated::test::{TestRequest, TestResponse};

// ============================================================================
// Test infrastructure
// ============================================================================

/// One responder replica: its own NATS connection handle, a label the
/// handlers echo back so queue-group distribution is observable, and a record
/// of every request the handlers saw.
struct RrCtx {
    nats: async_nats::Client,
    replica: String,
    handled: Mutex<Vec<u64>>,
}

impl HasNatsClient for RrCtx {
    fn nats_client(&self) -> &async_nats::Client {
        &self.nats
    }
}

impl RrCtx {
    fn record(&self, value: u64) {
        self.handled.lock().unwrap().push(value);
    }

    fn snapshot(&self) -> Vec<u64> {
        self.handled.lock().unwrap().clone()
    }
}

async fn rr_ctx(replica: &str) -> Arc<RrCtx> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "localhost:4222".to_string());
    let nats = async_nats::connect(&url)
        .await
        .unwrap_or_else(|e| panic!("cannot reach NATS at {url} (see module docs): {e}"));

    Arc::new(RrCtx {
        nats,
        replica: replica.to_string(),
        handled: Mutex::new(Vec::new()),
    })
}

const DEADLINE: Duration = Duration::from_secs(20);

/// Spawn the responder through the `WorkerInfo` that `#[nats_route]`
/// submitted, so the macro-generated registration is part of the path under
/// test rather than something the test reimplements.
fn spawn_route(subject: &str, ctx: &Arc<RrCtx>) -> tokio::task::JoinHandle<()> {
    let worker_name = format!("nats_route:{subject}");
    let info = flatbed::get_workers()
        .into_iter()
        .find(|w| w.name == worker_name)
        .unwrap_or_else(|| panic!("#[nats_route] must register a worker named '{worker_name}'"));

    let any: Arc<dyn Any + Send + Sync> = ctx.clone();
    tokio::spawn(async move {
        (info.worker)(any)
            .await
            .expect("responder exited with error");
    })
}

fn headers(content_type: &str, request_id: &str) -> NatsHeaderMap {
    let mut headers = NatsHeaderMap::new();
    headers.append("content-type", content_type);
    headers.append(REQUEST_ID_HEADER, request_id);
    headers
}

fn flatbuffers_request(value: u64, message: &str) -> Vec<u8> {
    TestRequest {
        message: Some(message.to_string()),
        value,
    }
    .to_flatbuffer()
}

async fn request(ctx: &RrCtx, subject: &str, headers: NatsHeaderMap, payload: Vec<u8>) -> Message {
    tokio::time::timeout(
        DEADLINE,
        ctx.nats
            .request_with_headers(subject.to_string(), headers, payload.into()),
    )
    .await
    .unwrap_or_else(|_| panic!("no reply on '{subject}' within {DEADLINE:?}"))
    .unwrap_or_else(|e| panic!("request to '{subject}' failed: {e}"))
}

fn reply_header<'a>(message: &'a Message, name: &str) -> Option<&'a str> {
    message
        .headers
        .as_ref()?
        .get(name)
        .map(async_nats::HeaderValue::as_str)
}

/// A subscription is only established once the SUB has reached the server, so
/// requests published before that are answered by nobody. An empty payload is
/// undecodable under either encoding, so the probe is answered with an error
/// reply without ever reaching a handler — readiness is observable without
/// polluting what the handlers recorded.
async fn wait_route_ready(ctx: &RrCtx, subject: &str) {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        let probe = tokio::time::timeout(
            Duration::from_millis(500),
            ctx.nats.request(subject.to_string(), Vec::new().into()),
        )
        .await;
        if matches!(probe, Ok(Ok(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no responder answered on '{subject}' within {DEADLINE:?}");
}

// ============================================================================
// FlatBuffers round trip
// ============================================================================

#[nats_route("flatbed.rr.echo")]
async fn echo(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value + 1,
        success: true,
    }))
}

/// A FlatBuffers request decodes into the handler's typed body and the reply
/// decodes back on the requester's side, under the FlatBuffers content type.
/// The requester's `x-request-id` rides through to the reply.
#[tokio::test]
#[ignore]
async fn responder_round_trips_a_flatbuffers_request() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.echo", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.echo").await;

    let reply = request(
        &ctx,
        "flatbed.rr.echo",
        headers("application/x-flatbuffers", "req-echo"),
        flatbuffers_request(41, "hello"),
    )
    .await;

    let decoded = TestResponse::from_flatbuffer(&reply.payload).expect("reply must be FlatBuffers");
    assert_eq!(
        decoded,
        TestResponse {
            message: Some("hello".to_string()),
            value: 42,
            success: true,
        }
    );
    assert_eq!(
        reply_header(&reply, "content-type"),
        Some("application/x-flatbuffers")
    );
    assert_eq!(reply_header(&reply, REQUEST_ID_HEADER), Some("req-echo"));
    assert_eq!(ctx.snapshot(), vec![41]);

    task.abort();
}

// ============================================================================
// JSON round trip
// ============================================================================

#[nats_route("flatbed.rr.json")]
async fn json_echo(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value,
        success: true,
    }))
}

/// A JSON request is decoded as JSON and answered as JSON — the reply
/// encoding follows the request's content type, not a server-side default.
#[tokio::test]
#[ignore]
async fn responder_answers_a_json_request_in_json() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.json", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.json").await;

    let payload = serde_json::to_vec(&TestRequest {
        message: Some("ping".to_string()),
        value: 7,
    })
    .unwrap();
    let reply = request(
        &ctx,
        "flatbed.rr.json",
        headers("application/json; charset=utf-8", "req-json"),
        payload,
    )
    .await;

    let decoded: TestResponse = serde_json::from_slice(&reply.payload).expect("reply must be JSON");
    assert_eq!(decoded.message.as_deref(), Some("ping"));
    assert_eq!(decoded.value, 7);
    assert_eq!(
        reply_header(&reply, "content-type"),
        Some("application/json")
    );
    assert_eq!(ctx.snapshot(), vec![7]);

    task.abort();
}

// ============================================================================
// Reply metadata the requester relies on
// ============================================================================

#[nats_route("flatbed.rr.tagged")]
async fn tagged(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        value: req.body.value,
        ..TestResponse::default()
    })
    .header("x-replica", &req.ctx.replica))
}

/// A requester that sends no `x-request-id` still gets a correlation id back,
/// and the handler's own response headers ride along with it.
#[tokio::test]
#[ignore]
async fn a_reply_carries_a_generated_request_id_and_the_handlers_headers() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.tagged", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.tagged").await;

    let mut content_type_only = NatsHeaderMap::new();
    content_type_only.append("content-type", "application/x-flatbuffers");
    let reply = request(
        &ctx,
        "flatbed.rr.tagged",
        content_type_only,
        flatbuffers_request(8, "anon"),
    )
    .await;

    let request_id = reply_header(&reply, REQUEST_ID_HEADER).expect("every reply is correlated");
    assert!(
        uuid::Uuid::parse_str(request_id).is_ok(),
        "an absent x-request-id is replaced by a generated uuid, got '{request_id}'"
    );
    assert_eq!(reply_header(&reply, "x-replica"), Some("solo"));

    task.abort();
}

// ============================================================================
// Wildcard subject tokens
// ============================================================================

#[nats_route("flatbed.rr.sat.{id}.call.{verb}")]
async fn satellite_call(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    let id = req.param("id").unwrap_or("<none>");
    let verb = req.param("verb").unwrap_or("<none>");
    Ok(Response::ok(TestResponse {
        message: Some(format!("{id}/{verb}")),
        value: req.body.value,
        success: req.param("missing").is_none(),
    }))
}

/// `{token}` segments subscribe as NATS wildcards and the matched tokens reach
/// the handler as request params, addressed by name.
#[tokio::test]
#[ignore]
async fn responder_captures_wildcard_subject_tokens_as_params() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.sat.{id}.call.{verb}", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.sat.x07.call.status").await;

    let reply = request(
        &ctx,
        "flatbed.rr.sat.x07.call.status",
        headers("application/x-flatbuffers", "req-wild"),
        flatbuffers_request(1, "query"),
    )
    .await;

    let decoded = TestResponse::from_flatbuffer(&reply.payload).unwrap();
    assert_eq!(decoded.message.as_deref(), Some("x07/status"));
    assert!(
        decoded.success,
        "an undeclared param name must not resolve to a value"
    );

    task.abort();
}

// ============================================================================
// Queue-group load balancing
// ============================================================================

#[nats_route("flatbed.rr.queue", queue = "flatbed-rr")]
async fn queued(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse {
        message: Some(req.ctx.replica.clone()),
        value: req.body.value,
        success: true,
    }))
}

/// Probes until both replicas have answered at least once, so the
/// load-balancing assertion doesn't race a subscription that hasn't reached
/// the server yet. Probe payloads carry value 0, which the count assertion
/// excludes.
async fn wait_both_replicas_ready(ctx: &RrCtx, subject: &str) {
    let start = Instant::now();
    let mut seen = HashSet::new();
    while start.elapsed() < DEADLINE {
        let probe = tokio::time::timeout(
            Duration::from_millis(500),
            ctx.nats.request_with_headers(
                subject.to_string(),
                headers("application/x-flatbuffers", "probe"),
                flatbuffers_request(0, "probe").into(),
            ),
        )
        .await;
        if let Ok(Ok(reply)) = probe {
            let decoded = TestResponse::from_flatbuffer(&reply.payload).unwrap();
            seen.insert(decoded.message.unwrap_or_default());
        }
        if seen.len() == 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("only {seen:?} answered on '{subject}' within {DEADLINE:?}");
}

/// Two replicas subscribed under one queue group split the request stream:
/// every request is handled exactly once across the pair. Without the queue
/// group each replica would answer every request, doubling the handled count.
#[tokio::test]
#[ignore]
async fn queue_group_hands_each_request_to_exactly_one_replica() {
    let one = rr_ctx("one").await;
    let two = rr_ctx("two").await;
    let first = spawn_route("flatbed.rr.queue", &one);
    let second = spawn_route("flatbed.rr.queue", &two);
    wait_both_replicas_ready(&one, "flatbed.rr.queue").await;

    const REQUESTS: u64 = 20;
    for value in 1..=REQUESTS {
        request(
            &one,
            "flatbed.rr.queue",
            headers("application/x-flatbuffers", "req-queue"),
            flatbuffers_request(value, "work"),
        )
        .await;
    }

    let mut handled: Vec<u64> = one
        .snapshot()
        .into_iter()
        .chain(two.snapshot())
        .filter(|value| *value != 0)
        .collect();
    handled.sort_unstable();

    assert_eq!(
        handled,
        (1..=REQUESTS).collect::<Vec<_>>(),
        "each request must be handled once across the queue group, never twice",
    );
    assert!(
        one.snapshot().iter().any(|v| *v != 0) && two.snapshot().iter().any(|v| *v != 0),
        "both replicas must take a share of the work",
    );

    first.abort();
    second.abort();
}

// ============================================================================
// Always-reply error semantics
// ============================================================================

#[nats_route("flatbed.rr.failing")]
async fn failing(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Err(FlatbedRouteError::not_found("no such satellite").code("NO_SUCH_SATELLITE"))
}

/// A handler error is answered, not dropped: the requester gets a reply
/// carrying the code, message, and status, so a timeout can only ever mean the
/// subject was unreachable.
#[tokio::test]
#[ignore]
async fn handler_error_is_answered_as_an_error_reply() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.failing", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.failing").await;

    let reply = request(
        &ctx,
        "flatbed.rr.failing",
        headers("application/x-flatbuffers", "req-fail"),
        flatbuffers_request(3, "query"),
    )
    .await;

    assert_eq!(
        reply_header(&reply, ERROR_CODE_HEADER),
        Some("NO_SUCH_SATELLITE")
    );
    assert_eq!(
        reply_header(&reply, ERROR_MESSAGE_HEADER),
        Some("no such satellite")
    );
    assert_eq!(reply_header(&reply, ERROR_STATUS_HEADER), Some("404"));
    assert_eq!(reply_header(&reply, REQUEST_ID_HEADER), Some("req-fail"));
    assert_eq!(ctx.snapshot(), vec![3]);

    task.abort();
}

#[nats_route("flatbed.rr.strict")]
async fn strict(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse::default()))
}

/// An undecodable payload never reaches the handler and is still answered —
/// with a decode error the requester can act on rather than a silent timeout.
#[tokio::test]
#[ignore]
async fn undecodable_payload_is_answered_with_a_decode_error() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.strict", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.strict").await;

    let reply = request(
        &ctx,
        "flatbed.rr.strict",
        headers("application/json", "req-bad"),
        b"not json at all".to_vec(),
    )
    .await;

    assert_eq!(
        reply_header(&reply, ERROR_CODE_HEADER),
        Some("DESERIALIZATION_ERROR")
    );
    assert_eq!(reply_header(&reply, ERROR_STATUS_HEADER), Some("400"));
    assert!(
        ctx.snapshot().is_empty(),
        "an undecodable payload must not reach the handler",
    );

    task.abort();
}

#[nats_route("flatbed.rr.panicking")]
async fn panicking(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    panic!("handler blew up");
}

/// A panicking handler is answered too, and the subscription survives it: a
/// dropped reply would be indistinguishable from an unreachable subject.
#[tokio::test]
#[ignore]
async fn panicking_handler_is_answered_and_the_responder_keeps_serving() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.panicking", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.panicking").await;

    let reply = request(
        &ctx,
        "flatbed.rr.panicking",
        headers("application/x-flatbuffers", "req-panic"),
        flatbuffers_request(5, "boom"),
    )
    .await;
    assert_eq!(reply_header(&reply, ERROR_CODE_HEADER), Some("PANIC"));
    assert_eq!(reply_header(&reply, ERROR_STATUS_HEADER), Some("500"));

    let second = request(
        &ctx,
        "flatbed.rr.panicking",
        headers("application/x-flatbuffers", "req-panic-2"),
        flatbuffers_request(6, "boom"),
    )
    .await;
    assert_eq!(reply_header(&second, ERROR_CODE_HEADER), Some("PANIC"));
    assert_eq!(ctx.snapshot(), vec![5, 6]);

    task.abort();
}

// ============================================================================
// Concurrent dispatch
// ============================================================================

#[nats_route("flatbed.rr.slow")]
async fn slow(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    tokio::time::sleep(HANDLER_DELAY).await;
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse {
        value: req.body.value,
        ..TestResponse::default()
    }))
}

const HANDLER_DELAY: Duration = Duration::from_millis(500);

/// Each message is dispatched on its own task, so a slow handler holds up only
/// its own reply. Serialized dispatch would take at least `n * HANDLER_DELAY`
/// for `n` in-flight requests.
#[tokio::test]
#[ignore]
async fn a_slow_handler_does_not_stall_the_subscription() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.slow", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.slow").await;

    const IN_FLIGHT: u64 = 5;
    let started = Instant::now();
    let replies = futures::future::join_all((1..=IN_FLIGHT).map(|value| {
        request(
            &ctx,
            "flatbed.rr.slow",
            headers("application/x-flatbuffers", "req-slow"),
            flatbuffers_request(value, "wait"),
        )
    }))
    .await;
    let elapsed = started.elapsed();

    assert_eq!(replies.len(), IN_FLIGHT as usize);
    assert!(
        elapsed < HANDLER_DELAY * 3,
        "{IN_FLIGHT} concurrent requests took {elapsed:?}; serialized dispatch would need at \
         least {:?}",
        HANDLER_DELAY * u32::try_from(IN_FLIGHT).unwrap(),
    );

    task.abort();
}

// ============================================================================
// Messages with nothing to reply to
// ============================================================================

#[nats_route("flatbed.rr.fireandforget")]
async fn fire_and_forget(
    req: Request<TestRequest, Arc<RrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.body.value);
    Ok(Response::ok(TestResponse {
        value: req.body.value,
        ..TestResponse::default()
    }))
}

/// A plain publish carries no reply subject. The responder has nowhere to
/// answer, so it drops the message and keeps serving the next request.
#[tokio::test]
#[ignore]
async fn a_message_without_a_reply_subject_does_not_stop_the_responder() {
    let ctx = rr_ctx("solo").await;
    let task = spawn_route("flatbed.rr.fireandforget", &ctx);
    wait_route_ready(&ctx, "flatbed.rr.fireandforget").await;

    ctx.nats
        .publish(
            "flatbed.rr.fireandforget".to_string(),
            flatbuffers_request(99, "dropped").into(),
        )
        .await
        .expect("publish failed");

    let reply = request(
        &ctx,
        "flatbed.rr.fireandforget",
        headers("application/x-flatbuffers", "req-after-drop"),
        flatbuffers_request(100, "answered"),
    )
    .await;

    let decoded = TestResponse::from_flatbuffer(&reply.payload).unwrap();
    assert_eq!(decoded.value, 100);
    assert_eq!(
        ctx.snapshot(),
        vec![100],
        "a message with no reply subject must not reach the handler",
    );

    task.abort();
}
