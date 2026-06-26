//! Integration tests for hyper-based flatbed server
//!
//! These tests verify:
//! - Route registration and validation
//! - JSON and FlatBuffer content negotiation
//! - HTTP methods (GET, POST, PUT, etc.)
//! - Error responses
//! - Header handling
//! - Telemetry endpoints
//! - OpenAPI endpoints
//! - Worker registration and execution

// Generated test types (plain structs as primary API)
#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

// Import plain structs
use generated::test::{TestRequest, TestResponse};

use flatbed::{
    get_routes, get_workers, route, validate_routes, FlatbedRouteError, FlatbedWorkerError,
    Request, Response, Worker,
};
use std::any::Any;
use std::sync::Arc;

/// Create a dummy context for test handler calls (no context needed)
fn dummy_ctx() -> Arc<dyn Any + Send + Sync> {
    Arc::new(())
}

// ============================================================================
// Test handlers using async pattern with Request<T> and Response<T>
// ============================================================================

#[route("/api/ping")]
async fn test_ping_handler(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    let msg = req.body.message.as_deref().unwrap_or("no message");

    Ok(Response::ok(TestResponse {
        message: Some(format!("Test pong: {}", msg)),
        value: req.body.value + 100,
        success: true,
    }))
}

#[route("/api/health")]
async fn test_health_handler(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: Some("Healthy".to_string()),
        value: req.body.value,
        success: true,
    }))
}

#[route("/api/error")]
async fn test_error_handler(
    _req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::bad_request("Test error message").code("TEST_ERROR"))
}

#[route("/api/resource", method = "GET", tag = "Resources")]
async fn test_get_handler(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: Some("Got resource".to_string()),
        value: req.body.value,
        success: true,
    }))
}

#[route("/api/resource", method = "PUT", tag = "Resources")]
async fn test_put_handler(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: Some("Updated resource".to_string()),
        value: req.body.value,
        success: true,
    }))
}

/// Handler that returns response with custom headers
#[route("/api/with-headers")]
async fn test_response_headers_handler(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse {
        message: Some("Response with headers".to_string()),
        value: req.body.value,
        success: true,
    })
    .header("x-custom-header", "custom-value")
    .header("x-another-header", "another-value"))
}

/// Handler that returns error with details
#[route("/api/error-with-details")]
async fn test_error_with_details_handler(
    _req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError<TestResponse>> {
    Err(FlatbedRouteError::bad_request("Validation failed")
        .code("VALIDATION_ERROR")
        .header("x-error-source", "validation")
        .with_details(TestResponse {
            message: Some("field 'email' is invalid".to_string()),
            value: 42,
            success: false,
        }))
}

/// Handler that returns error with custom headers
#[route("/api/error-with-headers")]
async fn test_error_with_headers_handler(
    _req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Err(FlatbedRouteError::not_found("Resource not found")
        .code("NOT_FOUND")
        .header("x-error-type", "not_found")
        .header("x-retry-after", "60"))
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn test_route_validation() {
    // Validate that all routes are unique
    let result = validate_routes();
    assert!(result.is_ok(), "Route validation should pass");
}

#[test]
fn test_route_registration() {
    let routes = get_routes();

    // Check that our test routes are registered
    assert!(
        routes.contains_key(&("/api/ping".to_string(), flatbed::HttpMethod::Post)),
        "Route POST /api/ping should be registered"
    );
    assert!(
        routes.contains_key(&("/api/health".to_string(), flatbed::HttpMethod::Post)),
        "Route POST /api/health should be registered"
    );

    // Verify route info including method
    let ping_route = routes
        .get(&("/api/ping".to_string(), flatbed::HttpMethod::Post))
        .unwrap();
    assert_eq!(ping_route.path, "/api/ping");
    assert_eq!(ping_route.method, flatbed::HttpMethod::Post);
    assert_eq!(ping_route.request_type, "TestRequest");

    // Verify GET method route
    let get_route = routes
        .get(&("/api/resource".to_string(), flatbed::HttpMethod::Get))
        .unwrap();
    assert_eq!(get_route.method, flatbed::HttpMethod::Get);

    // Verify PUT method route on same path
    let put_route = routes
        .get(&("/api/resource".to_string(), flatbed::HttpMethod::Put))
        .unwrap();
    assert_eq!(put_route.method, flatbed::HttpMethod::Put);
}

#[tokio::test]
async fn test_hyper_handler_json() {
    // Test the generated hyper handler directly
    let request = TestRequest {
        message: Some("Integration test message".to_string()),
        value: 9999,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();

    // Build request parts
    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/ping".to_string());

    // Call the handler
    let result =
        __hyper_handler_test_ping_handler(parts, request_bytes, "application/json", dummy_ctx())
            .await;
    assert!(result.is_ok(), "Handler should succeed");

    let response_parts = result.unwrap();
    assert_eq!(response_parts.status, flatbed::StatusCode::OK);
    assert_eq!(response_parts.content_type, "application/json");

    // Parse response
    let response: TestResponse =
        flatbed::serde_json::from_slice(&response_parts.body).expect("Should deserialize response");
    assert!(response.success);
    assert_eq!(response.value, 9999 + 100);
    assert!(response
        .message
        .as_deref()
        .unwrap()
        .contains("Integration test message"));
}

#[tokio::test]
async fn test_hyper_handler_flatbuffer() {
    // Test with FlatBuffer content type
    let request = TestRequest {
        message: Some("FlatBuffer test".to_string()),
        value: 1234,
    };
    let request_bytes = request.to_flatbuffer();

    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/ping".to_string());

    let result = __hyper_handler_test_ping_handler(
        parts,
        request_bytes,
        "application/x-flatbuffers",
        dummy_ctx(),
    )
    .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(response_parts.content_type, "application/x-flatbuffers");

    // Parse response
    let response =
        TestResponse::from_flatbuffer(&response_parts.body).expect("Should deserialize response");
    assert!(response.success);
    assert_eq!(response.value, 1234 + 100);
}

#[tokio::test]
async fn test_error_response_json() {
    let request = TestRequest {
        message: Some("test".to_string()),
        value: 0,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();
    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/error".to_string());

    let result =
        __hyper_handler_test_error_handler(parts, request_bytes, "application/json", dummy_ctx())
            .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(response_parts.status, flatbed::StatusCode::BAD_REQUEST);

    // Parse error body
    let error_body: serde_json::Value =
        serde_json::from_slice(&response_parts.body).expect("Should deserialize error");
    assert_eq!(error_body["code"], "TEST_ERROR");
    assert_eq!(error_body["message"], "Test error message");
}

#[tokio::test]
async fn test_request_id_propagation() {
    let request = TestRequest {
        message: Some("test".to_string()),
        value: 0,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();

    let mut parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/ping".to_string());
    parts.headers.insert(
        "x-request-id",
        flatbed::HeaderValue::from_static("custom-request-id-123"),
    );
    parts = parts.with_request_id_from_header();

    let result =
        __hyper_handler_test_ping_handler(parts, request_bytes, "application/json", dummy_ctx())
            .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(
        response_parts
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
        Some("custom-request-id-123")
    );
}

#[tokio::test]
async fn test_response_with_custom_headers() {
    let request = TestRequest {
        message: Some("test".to_string()),
        value: 123,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();
    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/with-headers".to_string());

    let result = __hyper_handler_test_response_headers_handler(
        parts,
        request_bytes,
        "application/json",
        dummy_ctx(),
    )
    .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(
        response_parts
            .headers
            .get("x-custom-header")
            .and_then(|v| v.to_str().ok()),
        Some("custom-value")
    );
    assert_eq!(
        response_parts
            .headers
            .get("x-another-header")
            .and_then(|v| v.to_str().ok()),
        Some("another-value")
    );
}

#[tokio::test]
async fn test_error_with_details() {
    let request = TestRequest {
        message: Some("test".to_string()),
        value: 0,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();
    let parts =
        flatbed::RequestParts::new(flatbed::Method::POST, "/api/error-with-details".to_string());

    let result = __hyper_handler_test_error_with_details_handler(
        parts,
        request_bytes,
        "application/json",
        dummy_ctx(),
    )
    .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(response_parts.status, flatbed::StatusCode::BAD_REQUEST);

    // Verify error header
    assert_eq!(
        response_parts
            .headers
            .get("x-error-source")
            .and_then(|v| v.to_str().ok()),
        Some("validation")
    );

    // Parse error body
    let error_body: serde_json::Value =
        serde_json::from_slice(&response_parts.body).expect("Should deserialize error");
    assert_eq!(error_body["code"], "VALIDATION_ERROR");
    assert_eq!(error_body["message"], "Validation failed");
    assert!(error_body["details"].is_object());
    assert_eq!(error_body["details"]["message"], "field 'email' is invalid");
}

#[tokio::test]
async fn test_error_with_custom_headers() {
    let request = TestRequest {
        message: Some("test".to_string()),
        value: 0,
    };
    let request_bytes = flatbed::serde_json::to_vec(&request).unwrap();
    let parts =
        flatbed::RequestParts::new(flatbed::Method::POST, "/api/error-with-headers".to_string());

    let result = __hyper_handler_test_error_with_headers_handler(
        parts,
        request_bytes,
        "application/json",
        dummy_ctx(),
    )
    .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    assert_eq!(response_parts.status, flatbed::StatusCode::NOT_FOUND);

    assert_eq!(
        response_parts
            .headers
            .get("x-error-type")
            .and_then(|v| v.to_str().ok()),
        Some("not_found")
    );
    assert_eq!(
        response_parts
            .headers
            .get("x-retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("60")
    );
}

#[tokio::test]
async fn test_flatbuffer_hyphenated_content_type() {
    // Test that hyphenated content type (application/x-flat-buffers) works
    let request = TestRequest {
        message: Some("Hyphenated content type test".to_string()),
        value: 5678,
    };
    let request_bytes = request.to_flatbuffer();
    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/api/ping".to_string());

    let result = __hyper_handler_test_ping_handler(
        parts,
        request_bytes,
        "application/x-flat-buffers",
        dummy_ctx(),
    )
    .await;
    assert!(result.is_ok());

    let response_parts = result.unwrap();
    // Response uses non-hyphenated form
    assert_eq!(response_parts.content_type, "application/x-flatbuffers");

    let response =
        TestResponse::from_flatbuffer(&response_parts.body).expect("Should deserialize response");
    assert!(response.success);
    assert_eq!(response.value, 5678 + 100);
}

// ============================================================================
// Worker tests
// ============================================================================

/// Test context for workers
struct TestWorkerContext {
    #[allow(dead_code)]
    pub name: String,
}

/// Simple worker for testing registration
#[derive(Default)]
struct TestSimpleWorker;

impl Worker for TestSimpleWorker {
    type Context = TestWorkerContext;
    const NAME: &'static str = "test_simple_worker";

    fn run(&self, _ctx: Arc<Self::Context>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async move { Ok(()) })
    }
}

flatbed::register_worker!(TestSimpleWorker, TestWorkerContext);

/// Worker with custom name and description
#[derive(Default)]
struct TestCustomWorker;

impl Worker for TestCustomWorker {
    type Context = TestWorkerContext;
    const NAME: &'static str = "custom-worker";
    const DESCRIPTION: Option<&'static str> = Some("A worker with custom attributes");

    fn run(&self, _ctx: Arc<Self::Context>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async move { Ok(()) })
    }
}

flatbed::register_worker!(TestCustomWorker, TestWorkerContext);

#[test]
fn test_worker_registration() {
    let workers = get_workers();

    // Check that our test workers are registered
    let simple_worker = workers.iter().find(|w| w.name == "test_simple_worker");
    assert!(
        simple_worker.is_some(),
        "Simple worker should be registered"
    );
    assert!(simple_worker.unwrap().description.is_none());

    let custom_worker = workers.iter().find(|w| w.name == "custom-worker");
    assert!(
        custom_worker.is_some(),
        "Custom worker should be registered"
    );
    assert_eq!(
        custom_worker.unwrap().description,
        Some("A worker with custom attributes")
    );
}

#[tokio::test]
async fn test_worker_execution() {
    let ctx = Arc::new(TestWorkerContext {
        name: "test".to_string(),
    });

    // Find and execute the simple worker
    let workers = get_workers();
    let simple_worker = workers
        .iter()
        .find(|w| w.name == "test_simple_worker")
        .unwrap();

    // The worker function expects Arc<dyn Any + Send + Sync>
    let dyn_ctx: Arc<dyn std::any::Any + Send + Sync> = ctx;
    let result = (simple_worker.worker)(dyn_ctx).await;

    assert!(result.is_ok(), "Worker should execute successfully");
}

// ============================================================================
// Boot lifecycle tests
// ============================================================================

/// Stub telemetry service for integration tests
struct StubTelemetryService;

impl flatbed::TelemetryService for StubTelemetryService {
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
        _labels: Option<std::collections::HashMap<String, String>>,
    ) -> Result<Arc<dyn flatbed::telemetry::Counter<f64>>, flatbed::TelemetryError> {
        Ok(Arc::new(StubCounter))
    }

    fn register_u64_counter(
        &self,
        _name: &str,
        _help: &str,
        _labels: Option<std::collections::HashMap<String, String>>,
    ) -> Result<Arc<dyn flatbed::telemetry::Counter<u64>>, flatbed::TelemetryError> {
        Ok(Arc::new(StubCounter))
    }

    fn get_feed(&self) -> Result<String, flatbed::TelemetryError> {
        Ok(String::new())
    }

    fn service_name(&self) -> String {
        "stub".to_string()
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

/// Test that verifies the two-phase boot lifecycle:
/// - During boot: /healthz → 200, /readyz → 503, user routes → 503
/// - After boot:  /healthz → 200, /readyz → 200, user routes → 200
#[tokio::test]
async fn test_boot_lifecycle_probes() {
    use flatbed::{Flatbed, FlatbedConfig};
    use tokio::sync::oneshot;

    // Find a free port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let telemetry: Arc<dyn flatbed::TelemetryService> = Arc::new(StubTelemetryService);

    let config = FlatbedConfig::new("Test API")
        .host("127.0.0.1")
        .port(port)
        .with_telemetry(telemetry);

    // oneshot to control when boot completes
    let (boot_tx, boot_rx) = oneshot::channel::<()>();

    // Spawn the server
    let server_handle = tokio::spawn(async move {
        Flatbed::run(config, |_config| async move {
            // Wait until test signals us to complete boot
            let _ = boot_rx.await;
            Ok(()) // context type is ()
        })
        .await
    });

    let base = format!("http://127.0.0.1:{}", port);
    let client = reqwest::Client::new();

    // Wait for TCP port to accept connections
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    // --- Assert DURING boot ---

    // /healthz should return 200 (server is alive)
    let resp = client
        .get(format!("{}/healthz", base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "/healthz should be 200 during boot"
    );

    // /readyz should return 503 (not ready yet)
    let resp = client.get(format!("{}/readyz", base)).send().await.unwrap();
    assert_eq!(
        resp.status().as_u16(),
        503,
        "/readyz should be 503 during boot"
    );

    // User route should return 503 (not ready yet)
    let resp = client
        .post(format!("{}/api/ping", base))
        .header("content-type", "application/json")
        .body(r#"{"message":"test","value":1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        503,
        "User routes should be 503 during boot"
    );

    // --- Signal boot to complete ---
    boot_tx.send(()).unwrap();

    // Wait for ready signal to propagate
    for _ in 0..100 {
        let resp = client.get(format!("{}/readyz", base)).send().await.unwrap();
        if resp.status().as_u16() == 200 {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    // --- Assert AFTER boot ---

    // /healthz should still be 200
    let resp = client
        .get(format!("{}/healthz", base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "/healthz should be 200 after boot"
    );

    // /readyz should now be 200
    let resp = client.get(format!("{}/readyz", base)).send().await.unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "/readyz should be 200 after boot"
    );

    // User route should now work
    let resp = client
        .post(format!("{}/api/ping", base))
        .header("content-type", "application/json")
        .body(r#"{"message":"lifecycle test","value":42}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "User routes should be 200 after boot"
    );

    // Clean up: abort the server task (it would run forever otherwise)
    server_handle.abort();
}

#[test]
fn test_router_path_matching() {
    use flatbed::hyper::Router;

    let router = Router::new();

    // This test verifies the router can match paths correctly
    // Note: We don't have actual handlers registered for this test
    let methods = router.get_allowed_methods("/api/users");
    assert!(methods.is_empty()); // No routes added

    // Path matching with no routes returns None
    assert!(router.match_route("/api/ping", "GET").is_none());
}
