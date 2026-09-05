//! Broker-backed integration tests for the typed request helper: a
//! `typed_request` call round-tripping against a `#[nats_route]` responder on
//! a real NATS server.
//!
//! The tests are marked `#[ignore]` so the plain workspace test run stays
//! broker-free. `scripts/nats-broker.sh` starts a broker scoped to the
//! current worktree and prints its address:
//!
//! ```text
//! scripts/nats-broker.sh up
//! NATS_URL=$(scripts/nats-broker.sh url) \
//!   cargo test -p flatbed --features nats,openapi \
//!     --test nats_request_broker -- --ignored
//! ```
//!
//! `NATS_URL` defaults to `localhost:4222`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flatbed::nats::HasNatsClient;
use flatbed::{
    nats_route, FlatbedRouteError, NatsEncoding, NatsRequestError, NatsRequestExt, Request,
    Response, StatusCode,
};
use generated::test::{TestRequest, TestResponse};

// ============================================================================
// Test infrastructure
// ============================================================================

/// The responder's context: its NATS connection and a record of what the
/// handlers saw, so the request the helper actually put on the wire is
/// observable from the answering side.
struct TrCtx {
    nats: async_nats::Client,
    seen: Mutex<Vec<String>>,
}

impl HasNatsClient for TrCtx {
    fn nats_client(&self) -> &async_nats::Client {
        &self.nats
    }
}

impl TrCtx {
    fn record(&self, note: impl Into<String>) {
        self.seen.lock().unwrap().push(note.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

async fn tr_ctx() -> Arc<TrCtx> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "localhost:4222".to_string());
    let nats = async_nats::connect(&url)
        .await
        .unwrap_or_else(|e| panic!("cannot reach NATS at {url} (see module docs): {e}"));

    Arc::new(TrCtx {
        nats,
        seen: Mutex::new(Vec::new()),
    })
}

const DEADLINE: Duration = Duration::from_secs(20);

/// Spawn the responder through the `WorkerInfo` that `#[nats_route]`
/// submitted, so the macro-generated registration is part of the path under
/// test rather than something the test reimplements.
fn spawn_route(subject: &str, ctx: &Arc<TrCtx>) -> tokio::task::JoinHandle<()> {
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

/// A subscription is only established once the SUB has reached the server, so
/// requests published before that are answered by nobody. An empty payload is
/// undecodable under either encoding, so the probe is answered with an error
/// reply without ever reaching a handler — readiness is observable without
/// polluting what the handlers recorded.
async fn wait_route_ready(ctx: &TrCtx, subject: &str) {
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

fn query(value: u64, message: &str) -> TestRequest {
    TestRequest {
        message: Some(message.to_string()),
        value,
    }
}

// ============================================================================
// FlatBuffers round trip
// ============================================================================

#[nats_route("flatbed.tr.echo")]
async fn echo(
    req: Request<TestRequest, Arc<TrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx.record(req.request_id.clone());
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value + 1,
        success: true,
    }))
}

/// The default encoding is FlatBuffers in both directions: the handler
/// decodes the typed body the helper encoded, and the helper decodes the
/// reply into the type the call site binds. The `x-request-id` the caller
/// sets reaches the handler, so a caller-side trace id survives the hop.
#[tokio::test]
#[ignore]
async fn a_flatbuffers_request_round_trips_into_the_bound_response_type() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.echo", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.echo").await;

    let reply: TestResponse = ctx
        .nats
        .typed_request("flatbed.tr.echo", &query(41, "hello"))
        .header("x-request-id", "req-typed")
        .await
        .expect("the responder must answer");

    assert_eq!(
        reply,
        TestResponse {
            message: Some("hello".to_string()),
            value: 42,
            success: true,
        }
    );
    assert_eq!(ctx.snapshot(), vec!["req-typed".to_string()]);

    task.abort();
}

// ============================================================================
// JSON round trip
// ============================================================================

#[nats_route("flatbed.tr.json")]
async fn json_echo(
    req: Request<TestRequest, Arc<TrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    req.ctx
        .record(req.header("content-type").unwrap_or("<none>"));
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value * 2,
        success: true,
    }))
}

/// Asking in JSON is one builder call, and it changes both directions: the
/// responder reads the request as JSON and answers in JSON, which is what the
/// helper then decodes.
#[tokio::test]
#[ignore]
async fn a_json_request_is_answered_and_decoded_as_json() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.json", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.json").await;

    let reply: TestResponse = ctx
        .nats
        .typed_request("flatbed.tr.json", &query(21, "json"))
        .encoding(NatsEncoding::Json)
        .await
        .expect("the responder must answer");

    assert_eq!(reply.value, 42);
    assert_eq!(ctx.snapshot(), vec!["application/json".to_string()]);

    task.abort();
}

// ============================================================================
// Wildcard subjects
// ============================================================================

#[nats_route("flatbed.tr.sat.{id}.status")]
async fn satellite_status(
    req: Request<TestRequest, Arc<TrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: req.param("id").map(str::to_string),
        value: req.body.value,
        success: true,
    }))
}

/// The helper takes a concrete subject, so a responder subscribed under a
/// `{token}` pattern answers it with the token bound to what was asked for.
#[tokio::test]
#[ignore]
async fn a_concrete_subject_reaches_a_wildcard_responder_with_the_token_bound() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.sat.{id}.status", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.sat.x07.status").await;

    let reply: TestResponse = ctx
        .nats
        .typed_request("flatbed.tr.sat.x07.status", &query(1, "ping"))
        .await
        .expect("the responder must answer");

    assert_eq!(reply.message, Some("x07".to_string()));

    task.abort();
}

// ============================================================================
// Error replies
// ============================================================================

#[nats_route("flatbed.tr.failing")]
async fn failing(
    _req: Request<TestRequest, Arc<TrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::not_found("no such satellite")
        .code("NOT_FOUND")
        .header("x-trace", "t-9"))
}

/// A handler that rejects reaches the caller as the error the handler
/// returned — status, code, and message intact — rather than as a timeout,
/// and propagating it with `?` keeps that status.
#[tokio::test]
#[ignore]
async fn a_handler_rejection_arrives_as_the_error_the_handler_returned() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.failing", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.failing").await;

    let result: Result<TestResponse, NatsRequestError> = ctx
        .nats
        .typed_request("flatbed.tr.failing", &query(1, "ping"))
        .await;

    let NatsRequestError::Reply { subject, error } = result.expect_err("the handler rejects")
    else {
        panic!("a rejection must not be reported as a transport failure");
    };
    assert_eq!(subject, "flatbed.tr.failing");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.code, "NOT_FOUND");
    assert_eq!(error.message, "no such satellite");
    assert_eq!(
        error.headers.get("x-trace").and_then(|v| v.to_str().ok()),
        Some("t-9"),
        "the handler's own error headers reach the caller"
    );
    assert!(
        error.headers.get("content-type").is_none() && error.headers.get("x-request-id").is_none(),
        "an error propagated onto an HTTP response must not describe the NATS hop"
    );

    task.abort();
}

// ============================================================================
// Body-less replies
// ============================================================================

#[nats_route("flatbed.tr.ack")]
async fn ack(req: Request<TestRequest, Arc<TrCtx>>) -> Result<Response<()>, FlatbedRouteError> {
    req.ctx.record(req.body.message.clone().unwrap_or_default());
    Ok(Response::ok(()))
}

/// A subject that acknowledges without a body still has a type its caller can
/// bind, so the ack-only shape needs no escape hatch.
#[tokio::test]
#[ignore]
async fn a_body_less_reply_decodes_as_the_unit_type() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.ack", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.ack").await;

    let reply: () = ctx
        .nats
        .typed_request("flatbed.tr.ack", &query(1, "noted"))
        .await
        .expect("the responder must answer");

    assert_eq!(reply, ());
    assert_eq!(ctx.snapshot(), vec!["noted".to_string()]);

    task.abort();
}

// ============================================================================
// Unanswered subjects
// ============================================================================

#[nats_route("flatbed.tr.slow")]
async fn slow(
    req: Request<TestRequest, Arc<TrCtx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    tokio::time::sleep(Duration::from_secs(5)).await;
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value,
        success: true,
    }))
}

/// A subscribed-but-silent responder is a timeout, and the timeout is the
/// caller's, not the client's default.
#[tokio::test]
#[ignore]
async fn a_responder_that_does_not_answer_in_time_is_a_timeout() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.slow", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.slow").await;

    let started = Instant::now();
    let result: Result<TestResponse, NatsRequestError> = ctx
        .nats
        .typed_request("flatbed.tr.slow", &query(1, "ping"))
        .timeout(Duration::from_millis(300))
        .await;

    assert!(
        matches!(result, Err(NatsRequestError::Timeout { .. })),
        "expected a timeout, got {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the caller's timeout must bound the wait, not the handler's sleep"
    );

    task.abort();
}

/// Nothing subscribed is reported as an unreachable subject rather than as a
/// timeout, so a misrouted call is distinguishable from a slow one without
/// waiting out the clock.
#[tokio::test]
#[ignore]
async fn an_unanswered_subject_is_reported_as_having_no_responder() {
    let ctx = tr_ctx().await;

    let result: Result<TestResponse, NatsRequestError> = ctx
        .nats
        .typed_request("flatbed.tr.nobody", &query(1, "ping"))
        .timeout(Duration::from_secs(10))
        .await;

    assert!(
        matches!(result, Err(NatsRequestError::NoResponders { .. })),
        "expected no responders, got {result:?}"
    );
}

// ============================================================================
// Reply decoding
// ============================================================================

#[nats_route("flatbed.tr.raw")]
async fn raw(_req: Request<TestRequest, Arc<TrCtx>>) -> Result<Response<()>, FlatbedRouteError> {
    Ok(Response::raw(b"id,count\n1,2\n".to_vec(), "text/csv"))
}

/// A reply the response type cannot read is a decode failure naming the
/// subject, not a silently wrong value.
#[tokio::test]
#[ignore]
async fn a_reply_that_is_not_the_response_type_is_a_decode_failure() {
    let ctx = tr_ctx().await;
    let task = spawn_route("flatbed.tr.raw", &ctx);
    wait_route_ready(&ctx, "flatbed.tr.raw").await;

    let result: Result<TestResponse, NatsRequestError> = ctx
        .nats
        .typed_request("flatbed.tr.raw", &query(1, "ping"))
        .await;

    let NatsRequestError::Decode { subject, .. } = result.expect_err("CSV is not a TestResponse")
    else {
        panic!("expected a decode failure");
    };
    assert_eq!(subject, "flatbed.tr.raw");

    task.abort();
}
