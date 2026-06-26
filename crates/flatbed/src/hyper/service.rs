//! Hyper service implementation for flatbed
//!
//! Implements the `hyper::service::Service` trait for handling HTTP requests.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::Service;
use tokio::sync::{watch, RwLock};

use super::router::Router;
use crate::{Error, FlatbedConfig, HeaderMap, HeaderValue, Method, RequestParts, ResponseParts};

#[cfg(feature = "openapi")]
use crate::{get_latest_version, get_openapi_json_for_version, get_route_versions};

/// Context passed to handlers
///
/// Contains probe state receivers for health/readiness checks and optional
/// application context that becomes available after the boot function completes.
pub struct ServiceContext<C> {
    /// The router for path matching
    pub router: Arc<Router>,
    /// Health probe receiver (true = healthy)
    pub healthz_rx: watch::Receiver<bool>,
    /// Ready probe receiver (true = ready)
    pub ready_rx: watch::Receiver<bool>,
    /// User-provided application context (None until boot completes)
    pub context: Arc<RwLock<Option<Arc<C>>>>,
    /// Flatbed configuration
    pub config: FlatbedConfig,
}

impl<C> ServiceContext<C> {
    /// Check if the server is healthy
    pub fn is_healthy(&self) -> bool {
        *self.healthz_rx.borrow()
    }

    /// Check if the server is ready to accept requests
    pub fn is_ready(&self) -> bool {
        *self.ready_rx.borrow()
    }
}

impl<C> Clone for ServiceContext<C> {
    fn clone(&self) -> Self {
        Self {
            router: Arc::clone(&self.router),
            healthz_rx: self.healthz_rx.clone(),
            ready_rx: self.ready_rx.clone(),
            context: Arc::clone(&self.context),
            config: self.config.clone(),
        }
    }
}

/// The flatbed service that handles HTTP requests
pub struct FlatbedService<C> {
    ctx: ServiceContext<C>,
}

impl<C> FlatbedService<C> {
    /// Create a new flatbed service
    pub fn new(ctx: ServiceContext<C>) -> Self {
        Self { ctx }
    }
}

impl<C> Clone for FlatbedService<C> {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
        }
    }
}

impl<C: Clone + Send + Sync + 'static> Service<Request<Incoming>> for FlatbedService<C> {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let ctx = self.ctx.clone();
        Box::pin(async move { Ok(handle_request(req, ctx).await) })
    }
}

/// Handle an incoming HTTP request
async fn handle_request<C: Clone + Send + Sync + 'static>(
    req: Request<Incoming>,
    ctx: ServiceContext<C>,
) -> Response<Full<Bytes>> {
    // Extract method and path before consuming req
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|s| s.to_string());

    // Check for built-in endpoints first

    // Splash endpoint at GET /
    if let Some(response) = handle_splash_endpoint(&method, &path, &ctx.config) {
        return response;
    }

    #[cfg(feature = "telemetry")]
    if ctx.config.telemetry.is_some() {
        if let Some(response) = handle_telemetry_endpoint(&method, &path, &ctx) {
            return response;
        }
    }

    #[cfg(feature = "openapi")]
    if let Some(response) = handle_openapi_endpoint(&method, &path, &ctx.config) {
        return response;
    }

    // Return 503 for user routes until server is ready
    if !ctx.is_ready() {
        return build_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "BOOTING",
            "Server is starting up, please retry shortly",
        );
    }

    // Try to match a user-defined route
    let Some((route_entry, path_params)) = ctx.router.match_route(&path, &method) else {
        // Check if path exists but method is not allowed
        let allowed = ctx.router.get_allowed_methods(&path);
        if !allowed.is_empty() {
            return build_method_not_allowed(&allowed);
        }
        return build_not_found();
    };

    // Extract content type
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Validate content type
    let is_json = content_type.contains("application/json");
    let is_flatbuffer = content_type.contains("application/x-flatbuffers")
        || content_type.contains("application/x-flat-buffers");

    // For methods with body, require valid content type
    let needs_body = matches!(method.to_uppercase().as_str(), "POST" | "PUT" | "PATCH");
    if needs_body && !is_json && !is_flatbuffer {
        return build_unsupported_media_type();
    }

    // Copy headers before consuming req
    let mut headers = HeaderMap::new();
    for (key, value) in req.headers().iter() {
        if let Ok(val) = HeaderValue::try_from(value.as_bytes()) {
            if let Ok(name) = http::header::HeaderName::try_from(key.as_str()) {
                headers.insert(name, val);
            }
        }
    }

    // Read body (consumes req)
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => {
            return build_error_response(
                StatusCode::BAD_REQUEST,
                "BODY_READ_ERROR",
                &format!("Failed to read request body: {}", e),
            );
        }
    };

    // Build request parts
    let mut request_parts = RequestParts::new(
        Method::from_bytes(method.as_bytes()).unwrap_or(Method::POST),
        path.clone(),
    );

    request_parts.headers = headers;
    request_parts.path_params = path_params;

    // Parse query parameters
    if let Some(query) = query {
        request_parts.query_params = query
            .split('&')
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let key = parts.next()?;
                let value = parts.next().unwrap_or("");
                if key.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value.to_string()))
                }
            })
            .collect();
    }

    // Set request ID from header or keep generated one
    request_parts = request_parts.with_request_id_from_header();

    // Extract application context for route handlers
    let app_ctx: Arc<dyn std::any::Any + Send + Sync> = {
        let guard = ctx.context.read().await;
        match guard.as_ref() {
            Some(c) => c.clone(),
            None => Arc::new(()),
        }
    };

    // Call the handler
    let handler = route_entry.handler;
    match handler(request_parts.clone(), body_bytes, &content_type, app_ctx).await {
        Ok(response_parts) => build_success_response(response_parts),
        Err(e) => {
            // Determine appropriate status code based on error type
            let (status, code) = match &e {
                Error::DeserializationError(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
                Error::SerializationError(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "SERIALIZATION_ERROR")
                }
                Error::HandlerError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "HANDLER_ERROR"),
                Error::Custom(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            };
            build_error_response(status, code, &e.to_string())
        }
    }
}

/// Handle splash endpoint (GET /)
fn handle_splash_endpoint(
    method: &str,
    path: &str,
    config: &FlatbedConfig,
) -> Option<Response<Full<Bytes>>> {
    if method.to_uppercase() != "GET" || path != "/" {
        return None;
    }

    let splash = config.splash.as_ref()?;

    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from(splash.clone())))
            .unwrap(),
    )
}

/// Handle telemetry endpoints (/healthz, /readyz, /metrics)
#[cfg(feature = "telemetry")]
fn handle_telemetry_endpoint<C>(
    method: &str,
    path: &str,
    ctx: &ServiceContext<C>,
) -> Option<Response<Full<Bytes>>> {
    if method.to_uppercase() != "GET" {
        return None;
    }

    let telemetry = ctx.config.telemetry.as_ref()?;

    match path {
        "/healthz" => {
            if ctx.is_healthy() {
                Some(build_text_response(StatusCode::OK, "OK"))
            } else {
                Some(build_text_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Not Healthy",
                ))
            }
        }
        "/readyz" => {
            if ctx.is_ready() {
                Some(build_text_response(StatusCode::OK, "Ready"))
            } else {
                Some(build_text_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Not Ready",
                ))
            }
        }
        "/metrics" => match telemetry.get_feed() {
            Ok(feed) => Some(build_metrics_response(feed)),
            Err(e) => Some(build_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "METRICS_ERROR",
                &e.to_string(),
            )),
        },
        _ => None,
    }
}

/// Handle OpenAPI endpoints (/openapi.json)
#[cfg(feature = "openapi")]
fn handle_openapi_endpoint(
    method: &str,
    path: &str,
    config: &FlatbedConfig,
) -> Option<Response<Full<Bytes>>> {
    if method.to_uppercase() != "GET" {
        return None;
    }

    // Match /openapi.json (latest version)
    if path == "/openapi.json" {
        let version = get_latest_version();
        let json = get_openapi_json_for_version(config, &version);
        return Some(build_json_response(json));
    }

    // Match /v{version}/openapi.json
    let versions = get_route_versions();
    for version in &versions {
        let versioned_path = format!("/{}/openapi.json", version);
        if path == versioned_path {
            let json = get_openapi_json_for_version(config, version);
            return Some(build_json_response(json));
        }
    }

    None
}

// Response builders

fn build_success_response(parts: ResponseParts) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(parts.status);

    // Set content type
    builder = builder.header("content-type", parts.content_type);

    // Copy headers
    for (key, value) in parts.headers.iter() {
        if let Ok(val) = value.to_str() {
            builder = builder.header(key.as_str(), val);
        }
    }

    builder
        .body(Full::new(Bytes::from(parts.body)))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from("Failed to build response")))
                .unwrap()
        })
}

fn build_not_found() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from("Not Found")))
        .unwrap()
}

fn build_method_not_allowed(allowed: impl AsRef<[String]>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header("content-type", "text/plain")
        .header("allow", allowed.as_ref().join(", "))
        .body(Full::new(Bytes::from("Method Not Allowed")))
        .unwrap()
}

fn build_unsupported_media_type() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from(
            "Content-Type must be application/json or application/x-flatbuffers",
        )))
        .unwrap()
}

fn build_error_response(status: StatusCode, code: &str, message: &str) -> Response<Full<Bytes>> {
    let body = serde_json::json!({
        "code": code,
        "message": message
    });

    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

#[cfg(feature = "telemetry")]
fn build_text_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

#[cfg(feature = "telemetry")]
fn build_metrics_response(metrics: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(Full::new(Bytes::from(metrics)))
        .unwrap()
}

#[cfg(feature = "openapi")]
fn build_json_response(json: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(json)))
        .unwrap()
}
