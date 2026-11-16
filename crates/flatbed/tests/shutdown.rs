//! On `SIGTERM` the server drops readiness, lets in-flight replies finish
//! inside `shutdown_timeout`, cuts what is still open past it, and flushes
//! the `ErrorReporter` once more before `serve` returns.
//!
//! The test signals its own process, so this binary holds a single test.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flatbed::bytes::Bytes;
use flatbed::hyper::{build_router, AutoServer, ServiceContext};
use flatbed::{
    route, ErrorReporter, FlatbedConfig, FlatbedRouteError, PanicReport, Readiness, Request,
    Response, RouteFailure, WorkerFailure,
};
use futures::StreamExt;
use generated::test::Defaulted;
use tokio::sync::{watch, RwLock};

const BUDGET_SECS: u64 = 2;

static FOREVER_DROPPED: AtomicBool = AtomicBool::new(false);

struct DropGuard;

impl Drop for DropGuard {
    fn drop(&mut self) {
        FOREVER_DROPPED.store(true, Ordering::SeqCst);
    }
}

type Chunks = futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>;

/// `first`, then after `gap` either `last` or, when `last` is `None`, an `Err`.
fn after_a_gap(first: &'static str, gap: Duration, last: Option<&'static str>) -> Chunks {
    let tail = futures::stream::once(async move {
        tokio::time::sleep(gap).await;
        last.map(|last| Bytes::from_static(last.as_bytes()))
            .ok_or_else(|| std::io::Error::other("upstream reset"))
    });
    futures::stream::once(async move { Ok(Bytes::from_static(first.as_bytes())) })
        .chain(tail)
        .boxed()
}

#[route("/short", method = "GET")]
async fn short(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let body = after_a_gap("start ", Duration::from_millis(800), Some("end"));
    Ok(Response::stream("text/plain", body))
}

#[route("/fails", method = "GET")]
async fn fails(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let body = after_a_gap("start ", Duration::from_millis(500), None);
    Ok(Response::stream("text/plain", body))
}

#[route("/forever", method = "GET")]
async fn forever(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let guard = DropGuard;
    let first = futures::stream::once(async { Ok(Bytes::from_static(b"start ")) });
    let never = futures::stream::pending().map(move |()| {
        let _held = &guard;
        Ok(Bytes::new())
    });
    Ok(Response::stream(
        "text/plain",
        first.chain(never).boxed() as Chunks,
    ))
}

/// Holds reports until `flush`, like a reporter with a background transport.
#[derive(Default)]
struct Buffered {
    pending: Mutex<Vec<String>>,
    delivered: Mutex<Vec<String>>,
}

impl ErrorReporter for Buffered {
    fn route_error(&self, failure: &RouteFailure<'_>) {
        let report = format!(
            "{} {} {}",
            failure.request.request_id,
            failure.status,
            failure.code.unwrap_or("-")
        );
        self.pending.lock().unwrap().push(report);
    }

    fn panic(&self, _panic: &PanicReport<'_>) {}

    fn worker_failure(&self, _failure: &WorkerFailure<'_>) {}

    fn flush(&self, _timeout: Duration) -> bool {
        let sent = std::mem::take(&mut *self.pending.lock().unwrap());
        self.delivered.lock().unwrap().extend(sent);
        true
    }
}

async fn open(client: &reqwest::Client, port: u16, path: &str) -> reqwest::Response {
    let mut answer = client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("x-request-id", path.trim_start_matches('/'))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), 200, "{path}");
    let first = answer.chunk().await.unwrap().unwrap();
    assert_eq!(first, "start ", "{path}");
    answer
}

/// The rest of the body, or `Err` when the transfer was cut.
async fn rest(mut answer: reqwest::Response) -> Result<String, reqwest::Error> {
    let mut body = String::new();
    while let Some(chunk) = answer.chunk().await? {
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
    Ok(body)
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_holds_in_flight_replies_up_to_the_budget() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let reporter = Arc::new(Buffered::default());
    let readiness = Readiness::new();
    let config = FlatbedConfig::new("shutdown")
        .readiness(readiness.clone())
        .with_error_reporter(reporter.clone());
    let (healthz_tx, healthz_rx) = watch::channel(true);
    let (_booted_tx, booted_rx) = watch::channel(true);
    let service_ctx = ServiceContext {
        router: Arc::new(build_router()),
        healthz_rx,
        booted_rx,
        context: Arc::new(RwLock::new(Some(Arc::new(())))),
        config,
        static_routes: Arc::new(Vec::new()),
    };
    let server = AutoServer::new(([127, 0, 0, 1], port).into(), service_ctx, healthz_tx)
        .shutdown_timeout(BUDGET_SECS);
    let serving = tokio::spawn(server.serve());

    let client = reqwest::Client::new();
    let readyz = format!("http://127.0.0.1:{port}/readyz");
    for _ in 0..100 {
        if client
            .get(&readyz)
            .send()
            .await
            .is_ok_and(|r| r.status() == 200)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let short = open(&client, port, "/short").await;
    let fails = open(&client, port, "/fails").await;
    let forever = open(&client, port, "/forever").await;

    let signalled = Instant::now();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());

    while readiness.is_ready() {
        assert!(
            signalled.elapsed() < Duration::from_secs(1),
            "readiness held after SIGTERM"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(readiness.blocked_on(), ["shutdown"]);

    assert_eq!(rest(short).await.unwrap(), "end");
    assert!(rest(fails).await.is_err(), "a failed stream is aborted");
    assert!(
        rest(forever).await.is_err(),
        "a stream past the budget is cut"
    );
    let cut_after = signalled.elapsed();
    assert!(
        cut_after >= Duration::from_secs(BUDGET_SECS) - Duration::from_millis(100),
        "cut after {cut_after:?}, before the budget"
    );

    tokio::time::timeout(Duration::from_secs(BUDGET_SECS + 5), serving)
        .await
        .expect("serve returned")
        .unwrap()
        .unwrap();
    assert!(FOREVER_DROPPED.load(Ordering::SeqCst));
    assert_eq!(
        *reporter.delivered.lock().unwrap(),
        ["fails 200 STREAM_ERROR"]
    );
}
