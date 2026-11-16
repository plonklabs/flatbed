//! Hyper service implementation for flatbed
//!
//! Implements the `hyper::service::Service` trait for handling HTTP requests.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::service::Service;
use tokio::sync::{watch, RwLock};

use super::observed::{Observed, Site};
use super::router::Router;
use crate::error_reporting::{self, RouteFailure, RouteRequest, Transport};
use crate::{
    Error, FlatbedBody, FlatbedConfig, FlatbedRouteError, HeaderMap, HeaderName, HeaderValue,
    Method, ReplyBody, RequestParts, ResponseParts,
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
    type Response = Response<FlatbedBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let ctx = self.ctx.clone();
        Box::pin(async move { Ok(handle_request(req, ctx).await) })
    }
}

/// What [`dispatch`] answered: a response a built-in tier finished itself,
/// or parts still to be finished into one, with the route they answer when
/// a route built them.
enum Reply {
    Built(Response<FlatbedBody>),
    Parts(ResponseParts, Option<Site>),
}

impl From<Response<FlatbedBody>> for Reply {
    fn from(response: Response<FlatbedBody>) -> Self {
        Self::Built(response)
    }
}

/// Handle an incoming HTTP request.
///
/// Dispatches to build the response, then enforces RFC 9110 §9.3.2 for HEAD:
/// the response carries the same headers a GET would but no body.
async fn handle_request<C: Clone + Send + Sync + 'static>(
    req: Request<Incoming>,
    ctx: ServiceContext<C>,
) -> Response<FlatbedBody> {
    let is_head = req.method() == http::Method::HEAD;
    match dispatch(req, ctx).await {
        Reply::Parts(parts, _) if is_head => {
            build_success_response(strip_parts_for_head(parts), None)
        }
        Reply::Parts(parts, site) => build_success_response(parts, site),
        Reply::Built(response) if is_head => strip_body_for_head(response).await,
        Reply::Built(response) => response,
    }
}

/// Strip a route reply's body for a HEAD request before it is finished.
///
/// A whole body's length becomes `content-length`. A stream is dropped
/// unpolled and no `content-length` is set: its length is unknown without
/// running it.
fn strip_parts_for_head(mut parts: ResponseParts) -> ResponseParts {
    if let ReplyBody::Full(bytes) = &parts.body {
        parts
            .headers
            .insert(http::header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    }
    parts.body = ReplyBody::Full(Vec::new());
    parts
}

/// Strip a built-in tier's response body for a HEAD request while preserving
/// its headers, setting `content-length` to the size the body would have had.
///
/// `hyper`'s HTTP/1.1 encoder already special-cases HEAD, but its HTTP/2
/// encoder does not: a response built with a real body is sent with one,
/// which a compliant HTTP/2 client rejects outright (the received data
/// contradicts the `content-length` a HEAD response is still expected to
/// carry). Stripping the body here, uniformly, keeps every transport
/// correct regardless of what a lower layer happens to already handle.
async fn strip_body_for_head(response: Response<FlatbedBody>) -> Response<FlatbedBody> {
    let (mut parts, body) = response.into_parts();
    let len = body
        .collect()
        .await
        .map_or(0, |collected| collected.to_bytes().len());
    parts
        .headers
        .insert(http::header::CONTENT_LENGTH, HeaderValue::from(len));
    Response::from_parts(parts, full(Bytes::new()))
}

/// Build the response for a request, including any body.
async fn dispatch<C: Clone + Send + Sync + 'static>(
    req: Request<Incoming>,
    ctx: ServiceContext<C>,
) -> Reply {
    // Extract method and path before consuming req
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|s| s.to_string());

    // Check for built-in endpoints first

    if let Some(response) = handle_splash_endpoint(&method, &path, &ctx.config) {
        return response.into();
    }

    #[cfg(feature = "telemetry")]
    if ctx.config.telemetry.is_some() {
        if let Some(response) = handle_telemetry_endpoint(&method, &path, &ctx) {
            return response.into();
        }
    }

    #[cfg(feature = "openapi")]
    if let Some(response) = handle_openapi_endpoint(&method, &path, &ctx.config) {
        return response.into();
    }

    if let Some(response) = handle_schema_endpoint(&method, &path) {
        return response.into();
    }

    if !ctx.is_ready() {
        let (code, message) = match ctx.blocked_gates() {
            Some(gates) => ("NOT_READY", format!("Waiting on {gates}")),
            None => (
                "BOOTING",
                "Server is starting up, please retry shortly".to_string(),
            ),
        };
        return build_error_response(StatusCode::SERVICE_UNAVAILABLE, code, &message).into();
    }

    // Try to match a user-defined route
    let Some((route_entry, path_params)) = ctx.router.match_route(&path, &method) else {
        // Check if path exists but method is not allowed
        let allowed = ctx.router.get_allowed_methods(&path);
        if !allowed.is_empty() {
            return build_method_not_allowed(&allowed).into();
        }
        if is_get_or_head(&method) && !ctx.static_routes.is_empty() {
            if let Some(parts) = super::static_files::serve(&ctx.static_routes, &path).await {
                return Reply::Parts(parts, None);
            }
        }
        return build_not_found().into();
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

    // Copy headers before consuming req
    let mut headers = HeaderMap::new();
    for (key, value) in req.headers().iter() {
        if let Ok(val) = HeaderValue::try_from(value.as_bytes()) {
            if let Ok(name) = http::header::HeaderName::try_from(key.as_str()) {
                headers.insert(name, val);
            }
        }
    }

    let respond_flatbuffer = match crate::accepted_codec(&headers) {
        Some(crate::Codec::Json) => false,
        Some(crate::Codec::FlatBuffer) => true,
        None => is_flatbuffer,
    };

    // Built ahead of the body read so an early rejection skips consuming it.
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

    let route = RouteRequest {
        transport: Transport::Http,
        method: Some(&method),
        route: route_entry.info.path,
        path: &path,
        request_id: &request_parts.request_id,
    };

    if let Some(hook) = ctx.config.before_request.as_ref() {
        if let Err(err) = hook(&request_parts) {
            if err.status.is_server_error() {
                error_reporting::route_error(&RouteFailure {
                    request: route,
                    status: err.status.as_u16(),
                    code: Some(wire_code(&err)),
                    message: &err.message,
                });
            }
            return build_route_error_response(&err, respond_flatbuffer, &request_parts.request_id)
                .into();
        }
    }

    // For methods with body, require valid content type
    let needs_body = matches!(method.to_uppercase().as_str(), "POST" | "PUT" | "PATCH");
    if needs_body && !is_json && !is_flatbuffer {
        return build_unsupported_media_type().into();
    }

    let limit = route_entry
        .info
        .max_body
        .unwrap_or(ctx.config.max_request_body);
    let too_large = || {
        FlatbedRouteError::with_status(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Request body exceeds {limit} bytes"),
        )
        .code("PAYLOAD_TOO_LARGE")
    };
    if declared_length(req.headers()).is_some_and(|length| length > limit) {
        return build_route_error_response(
            &too_large(),
            respond_flatbuffer,
            &request_parts.request_id,
        )
        .into();
    }

    // Read body (consumes req)
    let body_bytes = match Limited::new(req.into_body(), limit).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            let err = if e.downcast_ref::<LengthLimitError>().is_some() {
                too_large()
            } else {
                FlatbedRouteError::bad_request(format!("Failed to read request body: {e}"))
                    .code("BODY_READ_ERROR")
            };
            return build_route_error_response(&err, respond_flatbuffer, &request_parts.request_id)
                .into();
        }
    };

    // Extract application context for route handlers
    let app_ctx: Arc<dyn std::any::Any + Send + Sync> = {
        let guard = ctx.context.read().await;
        match guard.as_ref() {
            Some(c) => c.clone(),
            None => Arc::new(()),
        }
    };

    let handled = (route_entry.handler)(request_parts.clone(), body_bytes, &content_type, app_ctx);
    let e = match error_reporting::catch_panic(handled).await {
        Ok(Ok(response_parts)) => {
            let site = Site {
                method,
                route: route_entry.info.path,
                path,
                request_id: request_parts.request_id,
            };
            return Reply::Parts(response_parts, Some(site));
        }
        Ok(Err(e)) => e,
        Err(panic) => {
            error_reporting::route_panic(route, &panic);
            let err =
                FlatbedRouteError::internal("handler panicked").code(error_reporting::PANIC_CODE);
            return build_route_error_response(&err, respond_flatbuffer, &request_parts.request_id)
                .into();
        }
    };

    let (status, code) = match &e {
        Error::DeserializationError(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
        Error::SerializationError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SERIALIZATION_ERROR"),
        Error::HandlerError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "HANDLER_ERROR"),
        Error::Custom(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
    };
    let err = FlatbedRouteError::with_status(status, e.to_string()).code(code);
    if status.is_server_error() {
        error_reporting::route_error(&RouteFailure {
            request: route,
            status: status.as_u16(),
            code: Some(code),
            message: &err.message,
        });
    }
    build_route_error_response(&err, respond_flatbuffer, &request_parts.request_id).into()
}

/// The body length a request's `content-length` declares, when it has a
/// readable one.
fn declared_length(headers: &http::HeaderMap) -> Option<usize> {
    headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn is_get_or_head(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

fn handle_splash_endpoint(
    method: &str,
    path: &str,
    config: &FlatbedConfig,
) -> Option<Response<FlatbedBody>> {
    if !is_get_or_head(method) || path != "/" {
        return None;
    }

    let splash = config.splash.as_ref()?;

    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain; charset=utf-8")
            .body(full(Bytes::from(splash.clone())))
            .unwrap(),
    )
}

/// Handle telemetry endpoints (/healthz, /readyz, /metrics)
#[cfg(feature = "telemetry")]
fn handle_telemetry_endpoint<C>(
    method: &str,
    path: &str,
    ctx: &ServiceContext<C>,
) -> Option<Response<FlatbedBody>> {
    if !is_get_or_head(method) {
        return None;
    }

    let telemetry = ctx.config.telemetry.as_ref()?;

    match path {
        "/healthz" => {
            let (status, verdict) = if ctx.is_healthy() {
                (StatusCode::OK, "OK")
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, "Not Healthy")
            };
            let states = crate::supervisor::worker_states();
            Some(build_text_response(status, &health_body(verdict, &states)))
        }
        "/readyz" => {
            if ctx.is_ready() {
                Some(build_text_response(StatusCode::OK, "Ready"))
            } else {
                let body = match ctx.blocked_gates() {
                    Some(gates) => format!("Not Ready: {gates}"),
                    None => "Not Ready".to_string(),
                };
                Some(build_text_response(StatusCode::SERVICE_UNAVAILABLE, &body))
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

/// Append a line per supervised worker that is not running, so a probe
/// failure names the worker that caused it instead of only reporting that
/// the process is unhealthy.
#[cfg(feature = "telemetry")]
fn health_body(verdict: &str, states: &[(&'static str, crate::supervisor::WorkerState)]) -> String {
    let mut body = verdict.to_string();
    for (name, state) in states {
        if *state == crate::supervisor::WorkerState::Running {
            continue;
        }
        body.push('\n');
        body.push_str(name);
        body.push_str(": ");
        body.push_str(state.as_str());
    }
    body
}

/// Handle OpenAPI endpoints (/openapi.json)
#[cfg(feature = "openapi")]
fn handle_openapi_endpoint(
    method: &str,
    path: &str,
    config: &FlatbedConfig,
) -> Option<Response<FlatbedBody>> {
    if !is_get_or_head(method) {
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

/// Finish route or static-file parts into a response. A streamed body
/// answering a route is wrapped in [`Observed`].
fn build_success_response(parts: ResponseParts, site: Option<Site>) -> Response<FlatbedBody> {
    let mut builder = Response::builder().status(parts.status);

    // Set content type
    builder = builder.header("content-type", parts.content_type.as_ref());

    // Copy headers
    for (key, value) in parts.headers.iter() {
        if let Ok(val) = value.to_str() {
            builder = builder.header(key.as_str(), val);
        }
    }

    let body = match (parts.body, site) {
        (ReplyBody::Full(bytes), _) => full(bytes),
        (ReplyBody::Stream(body), Some(site)) => Observed::wrap(body, site, parts.status.as_u16()),
        (ReplyBody::Stream(body), None) => body,
    };
    builder.body(body).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(full(Bytes::from("Failed to build response")))
            .unwrap()
    })
}

fn full(bytes: impl Into<Bytes>) -> FlatbedBody {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn build_not_found() -> Response<FlatbedBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "text/plain")
        .body(full(Bytes::from("Not Found")))
        .unwrap()
}

fn build_method_not_allowed(allowed: impl AsRef<[String]>) -> Response<FlatbedBody> {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header("content-type", "text/plain")
        .header("allow", allowed.as_ref().join(", "))
        .body(full(Bytes::from("Method Not Allowed")))
        .unwrap()
}

fn build_unsupported_media_type() -> Response<FlatbedBody> {
    Response::builder()
        .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        .header("content-type", "text/plain")
        .body(full(Bytes::from(
            "Content-Type must be application/json or application/x-flatbuffers",
        )))
        .unwrap()
}

fn build_route_error_response(
    err: &FlatbedRouteError,
    is_flatbuffer: bool,
    request_id: &str,
) -> Response<FlatbedBody> {
    let error_code = wire_code(err);

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

    let mut builder = Response::builder()
        .status(err.status)
        .header("content-type", content_type);
    for (key, value) in headers.iter() {
        if let Ok(val) = value.to_str() {
            builder = builder.header(key.as_str(), val);
        }
    }
    builder.body(full(Bytes::from(body))).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(full(Bytes::from("Failed to build response")))
            .unwrap()
    })
}

/// The code put on the wire for `err`: its own, or `ERROR` when it set none.
fn wire_code(err: &FlatbedRouteError) -> &str {
    if err.code.is_empty() {
        "ERROR"
    } else {
        &err.code
    }
}

fn build_error_response(status: StatusCode, code: &str, message: &str) -> Response<FlatbedBody> {
    let body = serde_json::json!({
        "code": code,
        "message": message
    });

    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(full(Bytes::from(body.to_string())))
        .unwrap()
}

#[cfg(feature = "telemetry")]
fn build_text_response(status: StatusCode, body: &str) -> Response<FlatbedBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(full(Bytes::from(body.to_string())))
        .unwrap()
}

#[cfg(feature = "telemetry")]
fn build_metrics_response(metrics: String) -> Response<FlatbedBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(full(Bytes::from(metrics)))
        .unwrap()
}

#[cfg(feature = "openapi")]
fn build_json_response(json: String) -> Response<FlatbedBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(full(Bytes::from(json)))
        .unwrap()
}

fn handle_schema_endpoint(method: &str, path: &str) -> Option<Response<FlatbedBody>> {
    if !is_get_or_head(method) || path != "/schema.bfbs" {
        return None;
    }
    let Some(bfbs) = crate::get_schema_bfbs() else {
        return Some(build_not_found());
    };
    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .body(full(Bytes::from_static(bfbs)))
            .unwrap(),
    )
}

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use super::health_body;
    use crate::supervisor::WorkerState;

    #[test]
    fn health_body_is_the_bare_verdict_when_every_worker_runs() {
        let states = [("a", WorkerState::Running), ("b", WorkerState::Running)];
        assert_eq!(health_body("OK", &states), "OK");
    }

    #[test]
    fn health_body_names_only_the_workers_that_are_not_running() {
        let states = [
            ("healthy", WorkerState::Running),
            ("retrying", WorkerState::BackingOff),
            ("dead", WorkerState::Failed),
        ];
        assert_eq!(
            health_body("Not Healthy", &states),
            "Not Healthy\nretrying: backing-off\ndead: failed"
        );
    }
}
