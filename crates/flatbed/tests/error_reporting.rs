//! End-to-end check of HTTP error reporting through a real `Flatbed::run`: a
//! recording reporter must see every 5xx answer and caught panic, with the
//! route template, status, code, and request id, and never a 4xx.
//!
//! The reporter is process-wide, so every server in this binary shares one
//! recorder and each test picks out its reports by `x-request-id`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use flatbed::{
    route, ErrorReporter, Flatbed, FlatbedConfig, FlatbedRouteError, PanicReport, PanicSite,
    Request, Response, RouteFailure, RouteRequest, Transport, WorkerFailure,
};
use generated::test::{TestRequest, TestResponse};

#[derive(Debug, Clone, PartialEq)]
struct Route {
    transport: Transport,
    method: Option<String>,
    route: String,
    path: String,
    request_id: String,
}

impl From<&RouteRequest<'_>> for Route {
    fn from(request: &RouteRequest<'_>) -> Self {
        Self {
            transport: request.transport,
            method: request.method.map(str::to_string),
            route: request.route.to_string(),
            path: request.path.to_string(),
            request_id: request.request_id.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Record {
    RouteError {
        route: Route,
        status: u16,
        code: Option<String>,
        message: String,
    },
    Panic {
        route: Route,
        message: String,
        backtrace: bool,
    },
}

impl Record {
    fn request_id(&self) -> &str {
        match self {
            Record::RouteError { route, .. } | Record::Panic { route, .. } => &route.request_id,
        }
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

impl Recorder {
    fn for_request(&self, request_id: &str) -> Vec<Record> {
        let records = self.0.lock().unwrap();
        records
            .iter()
            .filter(|record| record.request_id() == request_id)
            .cloned()
            .collect()
    }
}

impl ErrorReporter for Recorder {
    fn route_error(&self, failure: &RouteFailure<'_>) {
        self.0.lock().unwrap().push(Record::RouteError {
            route: Route::from(&failure.request),
            status: failure.status,
            code: failure.code.map(str::to_string),
            message: failure.message.to_string(),
        });
    }

    fn panic(&self, panic: &PanicReport<'_>) {
        let PanicSite::Route(request) = panic.site else {
            return;
        };
        self.0.lock().unwrap().push(Record::Panic {
            route: Route::from(&request),
            message: panic.message.to_string(),
            backtrace: panic.backtrace.is_some(),
        });
    }

    fn worker_failure(&self, _failure: &WorkerFailure<'_>) {}
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);

#[route("/report/fail/{id}")]
async fn failing(_req: Request<TestRequest>) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::internal("database unreachable").code("DB_DOWN"))
}

#[route("/report/missing")]
async fn missing(_req: Request<TestRequest>) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::not_found("no such thing").code("NOPE"))
}

#[route("/report/panic")]
async fn panicking(
    _req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    panic!("handler blew up")
}

/// Start a server reporting to the shared recorder and wait until it is
/// ready, returning its base URL.
async fn boot(config: FlatbedConfig) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let reporter: Arc<dyn ErrorReporter> = RECORDER.clone();
    let config = config
        .host("127.0.0.1")
        .port(port)
        .with_error_reporter(reporter);
    tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let base = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if let Ok(response) = post(&base, "/report/missing", "readiness-probe").await {
            if response.status() != 503 {
                return base;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

async fn post(base: &str, path: &str, request_id: &str) -> reqwest::Result<reqwest::Response> {
    reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("x-request-id", request_id)
        .json(&serde_json::json!({ "message": "hi", "value": 1 }))
        .send()
        .await
}

fn http_route(route: &str, path: &str, request_id: &str) -> Route {
    Route {
        transport: Transport::Http,
        method: Some("POST".to_string()),
        route: route.to_string(),
        path: path.to_string(),
        request_id: request_id.to_string(),
    }
}

#[tokio::test]
async fn handler_server_error_is_reported_with_route_template_status_and_code() {
    let base = boot(FlatbedConfig::new("report-5xx")).await;

    let response = post(&base, "/report/fail/42", "req-5xx").await.unwrap();
    assert_eq!(response.status(), 500);

    assert_eq!(
        RECORDER.for_request("req-5xx"),
        vec![Record::RouteError {
            route: http_route("/report/fail/{id}", "/report/fail/42", "req-5xx"),
            status: 500,
            code: Some("DB_DOWN".to_string()),
            message: "database unreachable".to_string(),
        }]
    );
}

#[tokio::test]
async fn client_error_is_not_reported() {
    let base = boot(FlatbedConfig::new("report-4xx")).await;

    let response = post(&base, "/report/missing", "req-4xx").await.unwrap();
    assert_eq!(response.status(), 404);

    assert_eq!(RECORDER.for_request("req-4xx"), vec![]);
}

#[tokio::test]
async fn handler_panic_answers_500_panic_and_is_reported_once_with_a_backtrace() {
    let base = boot(FlatbedConfig::new("report-panic")).await;

    let response = post(&base, "/report/panic", "req-panic").await.unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(response.headers()["x-request-id"], "req-panic");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "PANIC");

    assert_eq!(
        RECORDER.for_request("req-panic"),
        vec![Record::Panic {
            route: http_route("/report/panic", "/report/panic", "req-panic"),
            message: "handler blew up".to_string(),
            backtrace: true,
        }]
    );
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn before_request_server_error_is_reported() {
    let config = FlatbedConfig::new("report-hook").before_request(|req| {
        if req.header("x-auth-backend") == Some("down") {
            return Err(FlatbedRouteError::internal("auth backend down").code("AUTH_DOWN"));
        }
        Ok(())
    });
    let base = boot(config).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/report/missing"))
        .header("x-request-id", "req-hook")
        .header("x-auth-backend", "down")
        .json(&serde_json::json!({ "message": "hi", "value": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);

    assert_eq!(
        RECORDER.for_request("req-hook"),
        vec![Record::RouteError {
            route: http_route("/report/missing", "/report/missing", "req-hook"),
            status: 500,
            code: Some("AUTH_DOWN".to_string()),
            message: "auth backend down".to_string(),
        }]
    );
}
