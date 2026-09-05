//! Byte-level golden transcript of every response the framework writes itself.
//!
//! Each tier the server can answer from — splash, telemetry, OpenAPI, schema,
//! the readiness 503, the router's 404/405, the content-type 415, and the
//! `#[route]` handler path in both codecs — is probed over a real socket and
//! rendered into one transcript compared against `golden/framework_endpoints.txt`.
//! The point is the envelope: status, the full header set, and the body bytes.
//!
//! Bodies whose content is generated elsewhere (the OpenAPI document, the
//! reflection schema) are recorded by size, and `date` and the request id are
//! normalized, so a mismatch means the pipeline changed what it writes.
//!
//! After an intentional change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p flatbed --test framework_golden`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::collections::HashMap;
use std::sync::Arc;

use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};
use generated::test::{TestRequest, TestResponse};

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/framework_endpoints.txt"
);

#[route("/api/echo")]
async fn echo(req: Request<TestRequest>) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: req.body.message.clone(),
        value: req.body.value,
        success: true,
    }))
}

#[route("/api/fail")]
async fn fail(_req: Request<TestRequest>) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::bad_request("echo refused").code("REFUSED"))
}

/// How much of a response body the transcript pins.
enum Body {
    /// The bytes themselves, as UTF-8.
    Exact,
    /// Only the length, for bodies produced by a generator this pipeline does
    /// not touch.
    Size,
}

struct Probe {
    method: &'static str,
    path: &'static str,
    content_type: Option<&'static str>,
    body: Vec<u8>,
    record: Body,
}

impl Probe {
    fn get(path: &'static str) -> Self {
        Self {
            method: "GET",
            path,
            content_type: None,
            body: Vec::new(),
            record: Body::Exact,
        }
    }

    fn post(path: &'static str, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            method: "POST",
            path,
            content_type: Some(content_type),
            body,
            record: Body::Exact,
        }
    }

    fn sized(mut self) -> Self {
        self.record = Body::Size;
        self
    }

    fn method(mut self, method: &'static str) -> Self {
        self.method = method;
        self
    }
}

fn sample_request() -> TestRequest {
    TestRequest {
        message: Some("golden".to_string()),
        value: 7,
    }
}

/// Probes answered identically whether or not the server has finished booting.
fn shared_probes() -> Vec<Probe> {
    vec![
        Probe::get("/"),
        Probe::get("/healthz"),
        Probe::get("/healthz").method("HEAD"),
        Probe::get("/metrics"),
        Probe::get("/openapi.json").sized(),
        Probe::get("/schema.bfbs").sized(),
        Probe::get("/readyz"),
    ]
}

/// Probes the readiness latch answers with a 503 before boot completes: one
/// that would route and one that would not.
fn latched_probes() -> Vec<Probe> {
    vec![
        Probe::get("/nope"),
        Probe::post(
            "/api/echo",
            "application/json",
            br#"{"message":"golden","value":7}"#.to_vec(),
        ),
    ]
}

/// Probes that reach past the readiness latch only once the server is ready.
fn routed_probes() -> Vec<Probe> {
    vec![
        Probe::get("/nope"),
        Probe::get("/api/echo"),
        Probe::post("/api/echo", "text/plain", b"ignored".to_vec()),
        Probe::post(
            "/api/echo",
            "application/json",
            br#"{"message":"golden","value":7}"#.to_vec(),
        ),
        Probe::post(
            "/api/echo",
            "application/x-flatbuffers",
            sample_request().to_flatbuffer(),
        )
        .sized(),
        Probe::post(
            "/api/fail",
            "application/json",
            br#"{"message":"golden","value":7}"#.to_vec(),
        ),
        Probe::post(
            "/api/fail",
            "application/x-flatbuffers",
            sample_request().to_flatbuffer(),
        ),
    ]
}

async fn render(client: &reqwest::Client, base: &str, probe: &Probe) -> String {
    let method = reqwest::Method::from_bytes(probe.method.as_bytes()).unwrap();
    let mut request = client.request(method, format!("{base}{}", probe.path));
    if let Some(content_type) = probe.content_type {
        request = request.header("content-type", content_type);
    }
    let response = request.body(probe.body.clone()).send().await.unwrap();

    let label = match probe.content_type {
        Some(content_type) => format!("{} {} [{}]", probe.method, probe.path, content_type),
        None => format!("{} {}", probe.method, probe.path),
    };

    let mut lines = vec![
        format!("### {label}"),
        response.status().as_u16().to_string(),
    ];

    let mut headers: Vec<String> = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "date")
        .map(|(name, value)| {
            let value = match name.as_str() {
                "x-request-id" => "<uuid>",
                _ => value.to_str().unwrap(),
            };
            format!("{name}: {value}")
        })
        .collect();
    headers.sort();
    lines.extend(headers);

    let body = response.bytes().await.unwrap();
    lines.push(match probe.record {
        Body::Exact => format!("body: {}", String::from_utf8_lossy(&body)),
        Body::Size => format!("body: <{} bytes>", body.len()),
    });

    lines.join("\n")
}

async fn transcript(client: &reqwest::Client, base: &str, probes: Vec<Probe>) -> Vec<String> {
    let mut sections = Vec::new();
    for probe in probes {
        sections.push(render(client, base, &probe).await);
    }
    sections
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn config(port: u16) -> FlatbedConfig {
    let telemetry: Arc<dyn flatbed::TelemetryService> = Arc::new(StubTelemetry);
    FlatbedConfig::new("Golden API")
        .host("127.0.0.1")
        .port(port)
        .splash("golden splash")
        .with_telemetry(telemetry)
}

async fn await_port(port: u16) {
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    panic!("server on port {port} never accepted a connection");
}

async fn await_ready(client: &reqwest::Client, base: &str) {
    for _ in 0..200 {
        if let Ok(response) = client.get(format!("{base}/readyz")).send().await {
            if response.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    panic!("server at {base} never became ready");
}

/// Every framework-written response, before and after the readiness latch,
/// against the committed transcript.
#[tokio::test]
async fn framework_endpoints_match_golden() {
    let booting_port = free_port();
    let booting = tokio::spawn(async move {
        Flatbed::run(config(booting_port), |_| async {
            std::future::pending::<()>().await;
            Ok(())
        })
        .await
    });

    let ready_port = free_port();
    let ready =
        tokio::spawn(async move { Flatbed::run(config(ready_port), |_| async { Ok(()) }).await });

    let client = reqwest::Client::new();
    let booting_base = format!("http://127.0.0.1:{booting_port}");
    let ready_base = format!("http://127.0.0.1:{ready_port}");

    await_port(booting_port).await;
    await_ready(&client, &ready_base).await;

    let mut sections = vec!["## booting".to_string()];
    sections.extend(transcript(&client, &booting_base, shared_probes()).await);
    sections.extend(transcript(&client, &booting_base, latched_probes()).await);
    sections.push("## ready".to_string());
    sections.extend(transcript(&client, &ready_base, shared_probes()).await);
    sections.extend(transcript(&client, &ready_base, routed_probes()).await);

    booting.abort();
    ready.abort();

    let actual = format!("{}\n", sections.join("\n\n"));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(std::path::Path::new(GOLDEN).parent().unwrap()).unwrap();
        std::fs::write(GOLDEN, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(GOLDEN).unwrap();
    assert_eq!(
        actual, expected,
        "framework responses drifted from the golden transcript; \
         re-run with UPDATE_GOLDEN=1 if the change is intended"
    );
}

struct StubTelemetry;

impl flatbed::TelemetryService for StubTelemetry {
    fn new(_config: flatbed::TelemetryConfig) -> Arc<Self>
    where
        Self: Sized,
    {
        Arc::new(Self)
    }

    fn register_f64_counter(
        &self,
        _name: &str,
        _help: &str,
        _labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn flatbed::telemetry::Counter<f64>>, flatbed::TelemetryError> {
        Ok(Arc::new(StubCounter))
    }

    fn register_u64_counter(
        &self,
        _name: &str,
        _help: &str,
        _labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn flatbed::telemetry::Counter<u64>>, flatbed::TelemetryError> {
        Ok(Arc::new(StubCounter))
    }

    fn get_feed(&self) -> Result<String, flatbed::TelemetryError> {
        Ok("# golden feed".to_string())
    }

    fn service_name(&self) -> String {
        "golden".to_string()
    }

    fn ip_address(&self) -> String {
        "127.0.0.1".to_string()
    }
}

struct StubCounter;

impl flatbed::telemetry::Counter<f64> for StubCounter {
    fn inc(&self) {}
    fn inc_by(&self, _value: f64) {}
}

impl flatbed::telemetry::Counter<u64> for StubCounter {
    fn inc(&self) {}
    fn inc_by(&self, _value: u64) {}
}
