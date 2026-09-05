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
use crate::{
    Error, FlatbedConfig, FlatbedRouteError, HeaderMap, HeaderName, HeaderValue, RequestParts,
    ResponseParts,
};

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
    /// Boot latch receiver (true once the boot function has returned a
    /// context). Readiness is this and every gate in `config.readiness`.
    pub booted_rx: watch::Receiver<bool>,
    /// User-provided application context (None until boot completes)
    pub context: Arc<RwLock<Option<Arc<C>>>>,
    /// Flatbed configuration
    pub config: FlatbedConfig,
    /// Static-file mounts registered via `static_route!`
    pub static_routes: Arc<Vec<crate::StaticRouteInfo>>,
}

impl<C> ServiceContext<C> {
    /// Check if the server is healthy
    pub fn is_healthy(&self) -> bool {
        *self.healthz_rx.borrow()
    }

    /// Check if the boot function has completed and the context is stored.
    ///
    /// This is the one-shot half of readiness: it never returns to `false`.
    pub fn is_booted(&self) -> bool {
        *self.booted_rx.borrow()
    }

    /// Check if the server is ready to accept requests: booted, and with
    /// every registered readiness gate reporting its dependency usable.
    pub fn is_ready(&self) -> bool {
        self.is_booted() && self.config.readiness.is_ready()
    }

    /// The gates holding readiness down as a comma-separated list, so a probe
    /// or an error can tell a lost dependency from a boot that has not
    /// finished.
    ///
    /// `None` covers both halves of the latter: a boot still running is
    /// waiting on itself, whatever gates it has registered along the way.
    fn blocked_gates(&self) -> Option<String> {
        if !self.is_booted() {
            return None;
        }

        let blocked = self.config.readiness.blocked_on();
        if blocked.is_empty() {
            return None;
        }

        Some(blocked.join(", "))
    }
}

impl<C> Clone for ServiceContext<C> {
    fn clone(&self) -> Self {
        Self {
            router: Arc::clone(&self.router),
            healthz_rx: self.healthz_rx.clone(),
            booted_rx: self.booted_rx.clone(),
            context: Arc::clone(&self.context),
            config: self.config.clone(),
            static_routes: Arc::clone(&self.static_routes),
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

/// Handle an incoming HTTP request.
///
/// Dispatches to build the response, then enforces RFC 9110 §9.3.2 for HEAD:
/// the response carries the same headers a GET would (including
/// `content-length`) but no body.
async fn handle_request<C: Clone + Send + Sync + 'static>(
    req: Request<Incoming>,
    ctx: ServiceContext<C>,
) -> Response<Full<Bytes>> {
    let is_head = req.method() == http::Method::HEAD;
    let response = dispatch(req, ctx).await;
    if is_head {
        strip_body_for_head(response).await
    } else {
        response
    }
}

/// Strip a response's body for a HEAD request while preserving its headers,
/// setting `content-length` to the size the body would have had.
///
/// `hyper`'s HTTP/1.1 encoder already special-cases HEAD, but its HTTP/2
/// encoder does not: a response built with a real body is sent with one,
/// which a compliant HTTP/2 client rejects outright (the received data
/// contradicts the `content-length` a HEAD response is still expected to
/// carry). Stripping the body here, uniformly, keeps every transport
/// correct regardless of what a lower layer happens to already handle.
async fn strip_body_for_head(response: Response<Full<Bytes>>) -> Response<Full<Bytes>> {
    let (mut parts, body) = response.into_parts();
    let Ok(collected) = body.collect().await;
    let len = collected.to_bytes().len();
    parts
        .headers
        .insert(http::header::CONTENT_LENGTH, HeaderValue::from(len));
    Response::from_parts(parts, Full::new(Bytes::new()))
}

/// Answer a request: type-erased parts in, type-erased parts out, hyper only
/// at the two ends.
async fn dispatch<C: Clone + Send + Sync + 'static>(
    req: Request<Incoming>,
    ctx: ServiceContext<C>,
) -> Response<Full<Bytes>> {
    let (head, body) = req.into_parts();
    finish(respond(request_parts(&head), body, ctx).await)
}

/// Build the request's type-erased parts, before any tier has looked at it.
fn request_parts(head: &http::request::Parts) -> RequestParts {
    let mut parts = RequestParts::new(head.method.clone(), head.uri.path().to_string());

    // Rebuilt rather than moved: `insert` collapses a repeated header name to
    // its last value, which is the single value `RequestParts::header` and
    // every handler reading `headers.get` are written against.
    for (key, value) in head.headers.iter() {
        if let Ok(val) = HeaderValue::try_from(value.as_bytes()) {
            if let Ok(name) = HeaderName::try_from(key.as_str()) {
                parts.headers.insert(name, val);
            }
        }
    }

    if let Some(query) = head.uri.query() {
        parts.query_params = query
            .split('&')
            .filter_map(|pair| {
                let mut fields = pair.splitn(2, '=');
                let key = fields.next()?;
                let value = fields.next().unwrap_or("");
                if key.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value.to_string()))
                }
            })
            .collect();
    }

    parts.with_request_id_from_header()
}

/// Render response parts onto the wire.
///
/// `content-type` is written from the dedicated field first, so parts carrying
/// one in `headers` too emit both, in that order.
fn finish(parts: ResponseParts) -> Response<Full<Bytes>> {
    let mut builder = Response::builder()
        .status(parts.status)
        .header("content-type", parts.content_type.as_ref());

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

/// The tier ladder: the first tier that claims the request answers it.
async fn respond<C: Clone + Send + Sync + 'static>(
    mut req: RequestParts,
    body: Incoming,
    ctx: ServiceContext<C>,
) -> ResponseParts {
    if let Some(parts) = splash_response(&req, &ctx.config) {
        return parts;
    }

    #[cfg(feature = "telemetry")]
    if let Some(parts) = telemetry_response(&req, &ctx) {
        return parts;
    }

    #[cfg(feature = "openapi")]
    if let Some(parts) = openapi_response(&req, &ctx.config) {
        return parts;
    }

    if let Some(parts) = schema_response(&req) {
        return parts;
    }

    if !ctx.is_ready() {
        return not_ready_response(&ctx);
    }

    let Some((route_entry, path_params)) = ctx.router.match_route(&req.path, req.method.as_str())
    else {
        let allowed = ctx.router.get_allowed_methods(&req.path);
        if !allowed.is_empty() {
            return method_not_allowed_response(&allowed);
        }
        if is_get_or_head(req.method.as_str()) && !ctx.static_routes.is_empty() {
            if let Some(parts) = super::static_files::serve(&ctx.static_routes, &req.path).await {
                return parts;
            }
        }
        return not_found_response();
    };
    req.path_params = path_params;

    let content_type = req.header("content-type").unwrap_or("").to_string();
    let is_json = content_type.contains("application/json");
    let is_flatbuffer = content_type.contains("application/x-flatbuffers")
        || content_type.contains("application/x-flat-buffers");

    if let Some(hook) = ctx.config.before_request.as_ref() {
        if let Err(err) = hook(&req) {
            return route_error_response(&err, is_flatbuffer, &req.request_id);
        }
    }

    let needs_body = matches!(
        req.method.as_str().to_uppercase().as_str(),
        "POST" | "PUT" | "PATCH"
    );
    if needs_body && !is_json && !is_flatbuffer {
        return unsupported_media_type_response();
    }

    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "BODY_READ_ERROR",
                &format!("Failed to read request body: {}", e),
            )
        }
    };

    let app_ctx: Arc<dyn std::any::Any + Send + Sync> = {
        let guard = ctx.context.read().await;
        match guard.as_ref() {
            Some(c) => c.clone(),
            None => Arc::new(()),
        }
    };

    let handler = route_entry.handler;
    match handler(req, body_bytes, &content_type, app_ctx).await {
        Ok(parts) => parts,
        Err(e) => handler_error_response(&e),
    }
}

fn is_get_or_head(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

fn splash_response(req: &RequestParts, config: &FlatbedConfig) -> Option<ResponseParts> {
    if !is_get_or_head(req.method.as_str()) || req.path != "/" {
        return None;
    }

    let splash = config.splash.as_ref()?;

    Some(ResponseParts::ok(
        splash.clone().into_bytes(),
        "text/plain; charset=utf-8",
    ))
}

/// Answer `/healthz`, `/readyz` and `/metrics`.
#[cfg(feature = "telemetry")]
fn telemetry_response<C>(req: &RequestParts, ctx: &ServiceContext<C>) -> Option<ResponseParts> {
    if !is_get_or_head(req.method.as_str()) {
        return None;
    }

    let telemetry = ctx.config.telemetry.as_ref()?;

    match req.path.as_str() {
        "/healthz" if ctx.is_healthy() => Some(text_response(StatusCode::OK, "OK")),
        "/healthz" => Some(text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Not Healthy",
        )),
        "/readyz" => Some(readyz_response(ctx)),
        "/metrics" => Some(match telemetry.get_feed() {
            Ok(feed) => ResponseParts::ok(
                feed.into_bytes(),
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "METRICS_ERROR",
                &e.to_string(),
            ),
        }),
        _ => None,
    }
}

#[cfg(feature = "telemetry")]
fn readyz_response<C>(ctx: &ServiceContext<C>) -> ResponseParts {
    if ctx.is_ready() {
        return text_response(StatusCode::OK, "Ready");
    }

    let body = match ctx.blocked_gates() {
        Some(gates) => format!("Not Ready: {gates}"),
        None => "Not Ready".to_string(),
    };
    text_response(StatusCode::SERVICE_UNAVAILABLE, &body)
}

/// Answer `/openapi.json` and its `/{version}/openapi.json` siblings.
#[cfg(feature = "openapi")]
fn openapi_response(req: &RequestParts, config: &FlatbedConfig) -> Option<ResponseParts> {
    if !is_get_or_head(req.method.as_str()) {
        return None;
    }

    let version = if req.path == "/openapi.json" {
        get_latest_version()
    } else {
        get_route_versions()
            .into_iter()
            .find(|version| req.path == format!("/{}/openapi.json", version))?
    };

    Some(ResponseParts::ok(
        get_openapi_json_for_version(config, &version).into_bytes(),
        "application/json",
    ))
}

fn schema_response(req: &RequestParts) -> Option<ResponseParts> {
    if !is_get_or_head(req.method.as_str()) || req.path != "/schema.bfbs" {
        return None;
    }

    let Some(bfbs) = crate::get_schema_bfbs() else {
        return Some(not_found_response());
    };

    Some(ResponseParts::ok(bfbs.to_vec(), "application/octet-stream"))
}

fn not_ready_response<C>(ctx: &ServiceContext<C>) -> ResponseParts {
    let (code, message) = match ctx.blocked_gates() {
        Some(gates) => ("NOT_READY", format!("Waiting on {gates}")),
        None => (
            "BOOTING",
            "Server is starting up, please retry shortly".to_string(),
        ),
    };
    error_response(StatusCode::SERVICE_UNAVAILABLE, code, &message)
}

fn not_found_response() -> ResponseParts {
    text_response(StatusCode::NOT_FOUND, "Not Found")
}

fn method_not_allowed_response(allowed: &[String]) -> ResponseParts {
    let mut parts = text_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
    if let Ok(val) = HeaderValue::try_from(allowed.join(", ")) {
        parts.headers.insert(HeaderName::from_static("allow"), val);
    }
    parts
}

fn unsupported_media_type_response() -> ResponseParts {
    text_response(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Content-Type must be application/json or application/x-flatbuffers",
    )
}

fn text_response(status: StatusCode, body: &str) -> ResponseParts {
    ResponseParts::with_status(body.as_bytes().to_vec(), status, "text/plain")
}

/// The framework's standard error shape: a JSON `{code, message}` body.
fn error_response(status: StatusCode, code: &str, message: &str) -> ResponseParts {
    let body = serde_json::json!({
        "code": code,
        "message": message
    })
    .to_string();

    ResponseParts::with_status(body.into_bytes(), status, "application/json")
}

fn handler_error_response(err: &Error) -> ResponseParts {
    let (status, code) = match err {
        Error::DeserializationError(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
        Error::SerializationError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SERIALIZATION_ERROR"),
        Error::HandlerError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "HANDLER_ERROR"),
        Error::Custom(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
    };
    error_response(status, code, &err.to_string())
}

/// Render a route error the way `#[route]` renders its own: a JSON body, or —
/// when the request negotiated FlatBuffers — code and message in headers over
/// an empty body.
fn route_error_response(
    err: &FlatbedRouteError,
    is_flatbuffer: bool,
    request_id: &str,
) -> ResponseParts {
    let error_code = if err.code.is_empty() {
        "ERROR"
    } else {
        &err.code
    };

    let mut headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::try_from(request_id) {
        headers.insert(HeaderName::from_static("x-request-id"), val);
    }
    for (key, value) in err.headers.iter() {
        headers.insert(key.clone(), value.clone());
    }

    let (body, content_type) = if is_flatbuffer {
        if let Ok(val) = HeaderValue::try_from(error_code) {
            headers.insert(HeaderName::from_static("x-error-code"), val);
        }
        if let Ok(val) = HeaderValue::try_from(err.message.as_str()) {
            headers.insert(HeaderName::from_static("x-error-message"), val);
        }
        (Vec::new(), "application/x-flatbuffers")
    } else {
        let body = serde_json::json!({
            "code": error_code,
            "message": err.message,
        })
        .to_string()
        .into_bytes();
        (body, "application/json")
    };

    ResponseParts {
        body,
        status: err.status,
        headers,
        content_type: content_type.into(),
    }
}
