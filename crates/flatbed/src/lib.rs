/// Flatbuffers utility library for Plonk
///
/// This library provides helper functions and utilities for working with
/// FlatBuffers in the Plonk ecosystem.
///
/// # Plain Struct API
///
/// The primary API uses plain Rust structs. Handlers work with regular structs:
///
/// ```rust,ignore
/// #[route("/ping")]
/// fn handle_ping(req: PingRequest) -> Result<PingResponse, Error> {
///     Ok(PingResponse {
///         message: Some(format!("pong: {}", req.message.unwrap_or_default())),
///         timestamp: req.timestamp,
///         success: true,
///     })
/// }
/// ```
///
/// FlatBuffer serialization is handled automatically by the framework.
use flatbuffers::FlatBufferBuilder;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
#[cfg(feature = "telemetry")]
use std::sync::Arc;
use std::sync::LazyLock;

// NATS JetStream support (enabled with "nats" feature)
#[cfg(feature = "nats")]
pub mod nats;
#[cfg(feature = "nats")]
pub use nats::{run_stream_worker, HasJetStream, NatsResult, StreamWorker};

// NATS JetStream KV cache worker (enabled with "nats" feature)
#[cfg(feature = "nats")]
pub mod kv;
#[cfg(feature = "nats")]
pub use kv::{run_kv_worker, KvWorker};

// Re-export async-nats for use in generated macro code
#[cfg(feature = "nats")]
#[doc(hidden)]
pub use async_nats;

// Re-export futures for use in generated macro code + by the k8s
// reconciler / watcher runtime executors.
#[cfg(any(feature = "nats", feature = "k8s"))]
#[doc(hidden)]
pub use futures;

// Kubernetes reconciler support. The `k8s` module itself only needs
// the kube client; the NATS-bound `KubeReconciler` trait and its
// runtime executor are further gated behind `feature = "nats"`
// inside the module.
#[cfg(feature = "k8s")]
pub mod k8s;
#[cfg(feature = "k8s")]
pub use k8s::{
    run_kube_native_reconciler, run_kube_watcher, wait_for_follower, wait_for_follower_loss,
    wait_for_leadership, wait_for_leadership_loss, HasKubeClient, HasLeaderElection,
    KubeNativeReconciler, KubeWatcher, ReconcileError,
};
#[cfg(all(feature = "nats", feature = "k8s"))]
pub use k8s::{run_kube_reconciler, KubeReconciler};

// Generic context wrapper (available when any service feature is enabled)
#[cfg(any(feature = "nats", feature = "k8s"))]
mod context;
#[cfg(any(feature = "nats", feature = "k8s"))]
pub use context::FlatbedContext;

// Re-export kube for use in generated macro code
#[cfg(feature = "k8s")]
#[doc(hidden)]
pub use kube;

// Hyper integration module
pub mod hyper;

// Telemetry module (enabled with "telemetry" feature)
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub use telemetry::{TelemetryConfig, TelemetryError, TelemetryService};

// Re-export dependencies so consumers don't need them directly
// The #[route] macro and generated types use these
pub use flatbuffers;
pub use serde;
pub use serde_json;
#[cfg(feature = "openapi")]
pub use utoipa;

// Re-export tokio for use in generated main macro code
#[doc(hidden)]
pub use tokio;

/// Boxed, Send future alias for trait return types.
///
/// Eliminates the need for `use std::future::Future; use std::pin::Pin;`
/// in every file that implements a Flatbed trait.
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

// Re-export http types so users don't need to add the dependency
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};

// Re-export uuid for use in generated code (request ID generation)
#[doc(hidden)]
pub use uuid;

// ============================================================================
// OpenAPI Support Types
// ============================================================================

/// Configuration for flatbed routes and optional telemetry integration.
///
/// This replaces the old `ApiInfo` struct and adds support for automatic
/// health endpoint registration when telemetry is enabled.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::{FlatbedConfig, configure_flatbed_routes};
///
/// // Basic configuration
/// let config = FlatbedConfig::new("My API")
///     .description("API description")
///     .external_url("https://api.example.com");
///
/// // With telemetry (registers /healthz, /readyz, /metrics automatically)
/// let config = FlatbedConfig::new("My API")
///     .with_telemetry(telemetry_service);
/// ```
#[derive(Clone)]
pub struct FlatbedConfig {
    /// API title for OpenAPI spec
    pub title: &'static str,
    /// API description for OpenAPI spec
    pub description: Option<&'static str>,
    /// External URL for OpenAPI spec (e.g., public endpoint through Envoy/TLS)
    pub external_url: Option<String>,
    /// Host address to bind to (default: "0.0.0.0")
    pub host: String,
    /// Port to bind to (default: 8080)
    pub port: u16,
    /// Splash message printed on server startup (None = no splash)
    pub splash: Option<String>,
    /// Telemetry service for health endpoints (when telemetry feature is enabled)
    #[cfg(feature = "telemetry")]
    pub telemetry: Option<Arc<dyn TelemetryService>>,
}

impl Default for FlatbedConfig {
    fn default() -> Self {
        Self {
            title: "API",
            description: None,
            external_url: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            splash: None,
            #[cfg(feature = "telemetry")]
            telemetry: None,
        }
    }
}

impl fmt::Debug for FlatbedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("FlatbedConfig");
        debug
            .field("title", &self.title)
            .field("description", &self.description)
            .field("external_url", &self.external_url)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("splash", &self.splash.as_ref().map(|_| "<splash>"));
        #[cfg(feature = "telemetry")]
        debug.field(
            "telemetry",
            &self.telemetry.as_ref().map(|_| "<TelemetryService>"),
        );
        debug.finish()
    }
}

impl FlatbedConfig {
    /// Create a new FlatbedConfig with the given API title.
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            description: None,
            external_url: None,
            host: "0.0.0.0".to_string(),
            port: 8080,
            splash: None,
            #[cfg(feature = "telemetry")]
            telemetry: None,
        }
    }

    /// Set the bind host address (default: "0.0.0.0").
    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    /// Set the bind port (default: 8080).
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the API description for OpenAPI spec.
    pub fn description(mut self, desc: &'static str) -> Self {
        self.description = Some(desc);
        self
    }

    /// Set a splash page returned at `GET /`.
    ///
    /// The splash is served as a plain text response when accessing the root URL.
    /// Use this to display API information, available endpoints, or ASCII art.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = FlatbedConfig::new("My API")
    ///     .host("0.0.0.0")
    ///     .port(8080)
    ///     .splash("My API Server\nEndpoints:\n  GET /healthz");
    /// ```
    pub fn splash(mut self, splash: impl Into<String>) -> Self {
        self.splash = Some(splash.into());
        self
    }

    /// Set the external URL for OpenAPI spec (e.g., public endpoint through Envoy/TLS).
    pub fn external_url(mut self, url: impl Into<String>) -> Self {
        self.external_url = Some(url.into());
        self
    }

    /// Enable telemetry with automatic health endpoint registration.
    ///
    /// When telemetry is enabled, the following endpoints are automatically registered:
    /// - `GET /healthz` - Health check endpoint (returns "OK")
    /// - `GET /readyz` - Readiness probe endpoint (returns "Ready")
    /// - `GET /metrics` - Prometheus metrics endpoint
    #[cfg(feature = "telemetry")]
    pub fn with_telemetry(mut self, telemetry: Arc<dyn TelemetryService>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Get the bind address as a SocketAddr string.
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

// ============================================================================
// Flatbed Server Runner
// ============================================================================

/// Main entry point for running a Flatbed server with two-phase boot pattern.
///
/// The server starts immediately and begins accepting connections. The framework
/// automatically manages health and readiness probes during the boot lifecycle:
///
/// 1. Server starts listening — `/healthz` returns 200 (alive)
/// 2. Boot function runs — `/readyz` returns 503, user routes return 503
/// 3. Boot completes, context is stored — `/readyz` returns 200, routes accept traffic
/// 4. Workers are spawned after the ready signal
///
/// If a worker fails, `/healthz` returns 503 to trigger Kubernetes restart.
///
/// # Context type
///
/// The context type `C` can be any `Clone + Send + Sync + 'static` type.
/// You have two options:
///
/// - **Custom struct**: Define your own struct and implement any required
///   traits (`HasJetStream`, `HasKubeClient`, `HasLeaderElection`) manually.
/// - **[`FlatbedContext<C>`]**: Use the provided wrapper which owns framework
///   clients and auto-implements the traits based on enabled features.
///   Your application data is accessed transparently via `Deref`.
///
/// # Example: custom context
///
/// ```rust,ignore
/// use flatbed::{Flatbed, FlatbedConfig};
///
/// #[derive(Clone)]
/// struct AppContext {
///     db: DatabasePool,
/// }
///
/// Flatbed::run(config, |_| async move {
///     Ok(AppContext { db: DatabasePool::connect().await? })
/// }).await?;
/// ```
///
/// # Example: FlatbedContext wrapper
///
/// ```rust,ignore
/// use flatbed::{Flatbed, FlatbedConfig, FlatbedContext};
///
/// #[derive(Clone)]
/// struct AppData { pub namespace: String }
/// type AppContext = FlatbedContext<AppData>;
///
/// Flatbed::run(config, |_| async move {
///     let ctx = FlatbedContext::builder(AppData { namespace: "default".into() })
///         .nats_client(nats_client)
///         .jetstream(jetstream)
///         .kube_client(kube_client)
///         .leader_election(is_leader_tx, is_leader_rx, ha_mode)
///         .build();
///     Ok(ctx)
/// }).await?;
/// ```
pub struct Flatbed;

impl Flatbed {
    /// Run the server with a boot function for two-phase initialization.
    ///
    /// The boot function receives a clone of the [`FlatbedConfig`] (for access to
    /// host, port, etc.) and returns the application context. The framework handles
    /// all probe signaling automatically:
    ///
    /// - `/healthz` → 200 as soon as the server starts listening
    /// - Boot function runs (probes: healthz=200, ready=503)
    /// - Context is stored, then `/readyz` → 200 and workers are spawned
    ///
    /// # Type Parameters
    ///
    /// - `C`: The application context type. Must be `Clone + Send + Sync + 'static`.
    /// - `F`: The boot function type.
    /// - `Fut`: The future returned by the boot function.
    ///
    /// # Arguments
    ///
    /// - `config`: Server configuration including host, port, and optional telemetry.
    /// - `boot`: Async function that receives a [`FlatbedConfig`] clone and returns
    ///   the application context.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Route validation fails (duplicate paths with different types)
    /// - The bind address is invalid
    /// - The server fails to start
    /// - The boot function returns an error
    pub async fn run<C, F, Fut>(config: FlatbedConfig, boot: F) -> Result<(), Error>
    where
        C: Clone + Send + Sync + 'static,
        F: FnOnce(FlatbedConfig) -> Fut + Send,
        Fut: std::future::Future<Output = Result<C, Box<dyn std::error::Error + Send + Sync>>>
            + Send,
    {
        use std::sync::Arc;
        use tokio::sync::{watch, RwLock};

        // Validate routes (fails fast on conflicts)
        crate::validate_routes().map_err(|e| Error::Custom(format!("Route conflict: {}", e)))?;

        // Build router from inventory-registered routes
        let router = Arc::new(hyper::build_router());

        // Parse bind address
        let addr: std::net::SocketAddr = config
            .bind_address()
            .parse()
            .map_err(|e| Error::Custom(format!("Invalid bind address: {}", e)))?;

        // Create watch channels for probes (start as unhealthy/not ready)
        let (healthz_tx, healthz_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = watch::channel(false);

        // Create shared context storage (None until boot completes)
        let context: Arc<RwLock<Option<Arc<C>>>> = Arc::new(RwLock::new(None));

        // Clone config for the boot function before moving original into ServiceContext
        let boot_config = config.clone();

        // Create ServiceContext with probe receivers
        let service_ctx = hyper::ServiceContext {
            router,
            healthz_rx,
            ready_rx,
            context: Arc::clone(&context),
            config,
        };

        // Create and spawn the server
        let server = hyper::AutoServer::new(addr, service_ctx, healthz_tx.clone());
        let server_handle = tokio::spawn(async move { server.serve().await });

        // Server is listening — mark as alive immediately
        // (prevents Kubernetes from killing pod during slow boot)
        let _ = healthz_tx.send(true);

        // Run boot function (receives config clone, returns context)
        let ctx = boot(boot_config)
            .await
            .map_err(|e| Error::Custom(format!("Boot error: {}", e)))?;

        // Store context FIRST, then signal ready
        // This fixes the race condition where workers could start before context was set
        {
            let mut guard = context.write().await;
            *guard = Some(Arc::new(ctx));
        }
        let _ = ready_tx.send(true);

        // Await server completion
        server_handle
            .await
            .map_err(|e| Error::Custom(format!("Server task error: {}", e)))?
            .map_err(|e| Error::Custom(format!("Server error: {}", e)))
    }
}

/// Schema field info for OpenAPI generation
#[derive(Clone, Copy, Debug)]
pub struct FieldInfo {
    pub name: &'static str,
    pub field_type: &'static str, // "string", "integer", "boolean", "number"
    pub fbs_type: &'static str,   // Original FBS type for description
    pub required: bool,
}

/// Schema information
#[derive(Clone, Copy, Debug)]
pub struct SchemaInfo {
    pub name: &'static str,
    pub fields: &'static [FieldInfo],
}

// ============================================================================
// Trait Definitions for JSON Companion Types
// ============================================================================

/// Trait for JSON companion types that can convert to FlatBuffer bytes
///
/// This trait is implemented by generated JSON companion structs (e.g., `PingRequestJson`)
/// and provides the interface for serialization and schema information.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::ToFlatBuffer;
///
/// // Generated companion type implements this trait
/// let json_req = PingRequestJson { message: Some("hello".into()), timestamp: 123 };
/// let bytes = json_req.to_flatbuffer();
/// ```
pub trait ToFlatBuffer: serde::de::DeserializeOwned + serde::Serialize {
    /// Schema fields for OpenAPI generation (as a const)
    const SCHEMA_FIELDS: &'static [FieldInfo];

    /// Schema name for OpenAPI generation (as a const)
    const SCHEMA_NAME: &'static str;

    /// Convert this JSON struct to FlatBuffer bytes
    fn to_flatbuffer(&self) -> Vec<u8>;

    /// Get schema field info for OpenAPI generation (runtime method)
    fn schema_fields() -> &'static [FieldInfo] {
        Self::SCHEMA_FIELDS
    }

    /// Get the schema name for OpenAPI (runtime method)
    fn schema_name() -> &'static str {
        Self::SCHEMA_NAME
    }
}

/// Implementation of ToFlatBuffer for unit type
///
/// This allows `FlatbedError<()>` (errors without details) to work with
/// the error response builder that requires ToFlatBuffer.
impl ToFlatBuffer for () {
    const SCHEMA_FIELDS: &'static [FieldInfo] = &[];
    const SCHEMA_NAME: &'static str = "Empty";

    fn to_flatbuffer(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// Trait connecting FlatBuffer types to their plain struct companions
///
/// This allows the `#[route]` macro to discover the plain struct type
/// without hardcoding module paths, making flatbed reusable across projects.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::HasJsonCompanion;
///
/// // Generated automatically by flatbed_build:
/// impl<'a> HasJsonCompanion for __fb::v_1::PingRequest<'a> {
///     type Json = PingRequest;  // Plain struct
/// }
/// ```
pub trait HasJsonCompanion {
    /// The plain struct companion type for this FlatBuffer type
    type Json: ToFlatBuffer;
}

/// Trait connecting plain structs to their FlatBuffer types
///
/// This provides bidirectional type mapping for the plain struct API.
///
/// # Example
///
/// ```rust,ignore
/// impl HasPlainStruct for PingRequest {
///     type Plain = PingRequest;
///     type FlatBuffer<'a> = __fb::v_1::PingRequest<'a>;
/// }
/// ```
pub trait HasPlainStruct {
    /// The plain struct type (usually Self)
    type Plain: ToFlatBuffer;
    /// The FlatBuffer view type
    type FlatBuffer<'a>;
}

/// OpenAPI metadata for a route
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenApiRouteInfo {
    pub version: Option<&'static str>, // e.g., "v1", "v2"
    pub tag: Option<&'static str>,
    pub summary: Option<&'static str>,
    pub operation_id: Option<&'static str>,
    pub deprecated: bool,
    pub request_schema: Option<SchemaInfo>,
    pub response_schema: Option<SchemaInfo>,
}

// Re-export the route macro from flatbed_macros
pub use flatbed_macros::route;

// Re-export the main macro from flatbed_macros
pub use flatbed_macros::main;

// Re-export inventory for use by the macro
#[doc(hidden)]
pub use inventory;

// ============================================================================
// HTTP Request/Response Wrapper Types
// ============================================================================

/// HTTP request wrapper providing context alongside the deserialized body
///
/// The generic parameter `T` is the request body type (flatbed-generated struct).
/// The optional parameter `C` is the application context type (defaults to `()`).
///
/// # Example
///
/// ```rust,ignore
/// // Simple handler without context
/// #[route("/ping")]
/// async fn handle_ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedError> {
///     Ok(Response::ok(PingResponse { message: Some("pong".into()), ... }))
/// }
///
/// // Handler with application context
/// #[route("/users")]
/// async fn handle_users(req: Request<UserRequest, AppCtx>) -> Result<Response<UserResponse>, FlatbedError> {
///     let user = req.ctx.db.find_user(&req.body.id).await?;
///     Ok(Response::ok(UserResponse { name: user.name, ... }))
/// }
/// ```
#[derive(Debug)]
pub struct Request<T, C = ()> {
    /// The deserialized request body
    pub body: T,
    /// Application context (zero-sized when C = ())
    pub ctx: C,
    /// HTTP headers from the request
    pub headers: HeaderMap,
    /// HTTP method
    pub method: Method,
    /// Request path
    pub path: String,
    /// Path parameters (e.g., /users/{id} -> {"id": "123"})
    pub path_params: HashMap<String, String>,
    /// Query string parameters
    pub query_params: HashMap<String, String>,
    /// Request ID (propagated or generated)
    pub request_id: String,
}

impl<T, C> Request<T, C> {
    /// Get a header value by name (case-insensitive)
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Get a path parameter by name
    pub fn param(&self, name: &str) -> Option<&str> {
        self.path_params.get(name).map(|s| s.as_str())
    }

    /// Get a query parameter by name
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query_params.get(name).map(|s| s.as_str())
    }

    /// Get the request ID
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl<T, C> Deref for Request<T, C> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.body
    }
}

/// HTTP response wrapper with status code and headers
///
/// # Example
///
/// ```rust,ignore
/// // Simple 200 OK response
/// Response::ok(MyResponse { ... })
///
/// // 201 Created
/// Response::created(MyResponse { ... })
///
/// // Custom status and headers
/// Response::with_status(MyResponse { ... }, StatusCode::ACCEPTED)
///     .header("X-Custom", "value")
/// ```
#[derive(Debug)]
pub struct Response<T> {
    /// The response body
    pub body: T,
    /// HTTP status code (default: 200 OK)
    pub status: StatusCode,
    /// Response headers
    pub headers: HeaderMap,
}

impl<T> Response<T> {
    /// Create a response with 200 OK status
    pub fn ok(body: T) -> Self {
        Self {
            body,
            status: StatusCode::OK,
            headers: HeaderMap::new(),
        }
    }

    /// Create a response with 201 Created status
    pub fn created(body: T) -> Self {
        Self {
            body,
            status: StatusCode::CREATED,
            headers: HeaderMap::new(),
        }
    }

    /// Create a response with 202 Accepted status
    pub fn accepted(body: T) -> Self {
        Self {
            body,
            status: StatusCode::ACCEPTED,
            headers: HeaderMap::new(),
        }
    }

    /// Create a response with 204 No Content status
    pub fn no_content(body: T) -> Self {
        Self {
            body,
            status: StatusCode::NO_CONTENT,
            headers: HeaderMap::new(),
        }
    }

    /// Create a response with custom status code
    pub fn with_status(body: T, status: StatusCode) -> Self {
        Self {
            body,
            status,
            headers: HeaderMap::new(),
        }
    }

    /// Set the HTTP status code (builder pattern)
    pub fn status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Add a header to the response (builder pattern)
    pub fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(val)) = (
            HeaderName::try_from(key.as_ref()),
            HeaderValue::try_from(value.as_ref()),
        ) {
            self.headers.insert(name, val);
        }
        self
    }
}

// ============================================================================
// Worker Error Type
// ============================================================================

/// Simple error type for background workers
///
/// Workers use this simpler error type since they don't need HTTP-specific
/// fields like status codes or headers.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::{worker, FlatbedWorkerError};
/// use std::sync::Arc;
///
/// #[worker]
/// async fn my_worker(ctx: Arc<AppContext>) -> Result<(), FlatbedWorkerError> {
///     ctx.do_something().await
///         .map_err(|e| FlatbedWorkerError::new("TASK_ERROR", e.to_string()))?;
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct FlatbedWorkerError {
    /// Application-defined error code (e.g., "CACHE_ERROR")
    pub code: String,
    /// Human-readable error message
    pub message: String,
}

impl FlatbedWorkerError {
    /// Create a new worker error with code and message
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for FlatbedWorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for FlatbedWorkerError {}

// ============================================================================
// HTTP Route Error Type
// ============================================================================

/// HTTP-conformant error type with status code, error code, and optional details
///
/// The generic parameter `D` is the type for structured error details (must be a
/// flatbed-generated struct implementing `ToFlatBuffer`). Defaults to `()` for
/// errors without structured details.
///
/// # Example
///
/// ```rust,ignore
/// // Simple error without details
/// return Err(FlatbedRouteError::bad_request("Invalid input"));
///
/// // Error with code
/// return Err(FlatbedRouteError::bad_request("Validation failed")
///     .code("VALIDATION_ERROR"));
///
/// // Error with typed details (flatbed-generated struct)
/// return Err(FlatbedRouteError::bad_request("Validation failed")
///     .code("VALIDATION_ERROR")
///     .with_details(ValidationErrorDetails {
///         field: Some("email".into()),
///         reason: Some("Invalid format".into()),
///     }));
/// ```
#[derive(Debug, Clone)]
pub struct FlatbedRouteError<D = ()> {
    /// HTTP status code
    pub status: StatusCode,
    /// Application-defined error code (e.g., "VALIDATION_ERROR")
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Response headers
    pub headers: HeaderMap,
    /// Optional structured error details
    pub details: Option<D>,
}

/// Deprecated alias for `FlatbedRouteError`
///
/// Use `FlatbedRouteError` instead for new code.
#[deprecated(since = "0.2.0", note = "Use FlatbedRouteError instead")]
pub type FlatbedError<D = ()> = FlatbedRouteError<D>;

impl FlatbedRouteError<()> {
    /// Create a 400 Bad Request error
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::BAD_REQUEST, message)
    }

    /// Create a 401 Unauthorized error
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::UNAUTHORIZED, message)
    }

    /// Create a 403 Forbidden error
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::FORBIDDEN, message)
    }

    /// Create a 404 Not Found error
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::NOT_FOUND, message)
    }

    /// Create a 409 Conflict error
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::CONFLICT, message)
    }

    /// Create a 422 Unprocessable Entity error
    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    /// Create a 500 Internal Server Error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Create an error with custom status code
    pub fn with_status(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code: String::new(),
            message: message.into(),
            headers: HeaderMap::new(),
            details: None,
        }
    }
}

impl<D> FlatbedRouteError<D> {
    /// Set the error code (builder pattern)
    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.code = code.into();
        self
    }

    /// Add a header to the error response (builder pattern)
    pub fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(val)) = (
            HeaderName::try_from(key.as_ref()),
            HeaderValue::try_from(value.as_ref()),
        ) {
            self.headers.insert(name, val);
        }
        self
    }

    /// Add structured details to the error, changing the type parameter
    pub fn with_details<D2>(self, details: D2) -> FlatbedRouteError<D2> {
        FlatbedRouteError {
            status: self.status,
            code: self.code,
            message: self.message,
            headers: self.headers,
            details: Some(details),
        }
    }

    /// Get the HTTP status code
    pub fn status_code(&self) -> StatusCode {
        self.status
    }
}

impl<D> fmt::Display for FlatbedRouteError<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.code.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "[{}] {}", self.code, self.message)
        }
    }
}

impl<D: fmt::Debug> std::error::Error for FlatbedRouteError<D> {}

/// Parts extracted from an HTTP request (used internally by macro)
#[derive(Debug, Clone)]
pub struct RequestParts {
    /// HTTP headers
    pub headers: HeaderMap,
    /// HTTP method
    pub method: Method,
    /// Request path
    pub path: String,
    /// Path parameters
    pub path_params: HashMap<String, String>,
    /// Query parameters
    pub query_params: HashMap<String, String>,
    /// Request ID (propagated or generated)
    pub request_id: String,
}

impl RequestParts {
    /// Create new RequestParts with generated request ID
    pub fn new(method: Method, path: String) -> Self {
        Self {
            headers: HeaderMap::new(),
            method,
            path,
            path_params: HashMap::new(),
            query_params: HashMap::new(),
            request_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Set request ID from header or keep generated one
    pub fn with_request_id_from_header(mut self) -> Self {
        if let Some(id) = self
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
        {
            self.request_id = id.to_string();
        }
        self
    }
}

/// Parts for constructing an HTTP response (used internally by macro)
#[derive(Debug)]
pub struct ResponseParts {
    /// Response body bytes
    pub body: Vec<u8>,
    /// HTTP status code
    pub status: StatusCode,
    /// Response headers
    pub headers: HeaderMap,
    /// Content-Type header value
    pub content_type: &'static str,
}

impl ResponseParts {
    /// Create new ResponseParts with 200 OK status
    pub fn ok(body: Vec<u8>, content_type: &'static str) -> Self {
        Self {
            body,
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            content_type,
        }
    }

    /// Create new ResponseParts with custom status
    pub fn with_status(body: Vec<u8>, status: StatusCode, content_type: &'static str) -> Self {
        Self {
            body,
            status,
            headers: HeaderMap::new(),
            content_type,
        }
    }

    /// Add X-Request-ID header
    pub fn with_request_id(mut self, request_id: &str) -> Self {
        if let Ok(val) = HeaderValue::try_from(request_id) {
            self.headers.insert("x-request-id", val);
        }
        self
    }
}

// ============================================================================
// Legacy Error Type
// ============================================================================

/// Error type for flatbed operations
#[derive(Debug, Clone)]
pub enum Error {
    /// Failed to deserialize FlatBuffer
    DeserializationError(String),
    /// Failed to serialize FlatBuffer
    SerializationError(String),
    /// Handler returned an error
    HandlerError(String),
    /// Custom error with message
    Custom(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DeserializationError(msg) => write!(f, "Deserialization error: {}", msg),
            Error::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Error::HandlerError(msg) => write!(f, "Handler error: {}", msg),
            Error::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<flatbuffers::InvalidFlatbuffer> for Error {
    fn from(err: flatbuffers::InvalidFlatbuffer) -> Self {
        Error::DeserializationError(err.to_string())
    }
}

impl From<String> for Error {
    fn from(msg: String) -> Self {
        Error::Custom(msg)
    }
}

impl From<&str> for Error {
    fn from(msg: &str) -> Self {
        Error::Custom(msg.to_string())
    }
}

/// Legacy response wrapper that contains a FlatBufferBuilder with a finalized response of type T
///
/// **Deprecated**: Use the new `Response<T>` struct instead, which works with plain structs.
///
/// This allows handlers to have type-safe signatures and include HTTP metadata:
/// ```ignore
/// fn handle(req: PingRequest) -> Result<LegacyResponse<PingResponse>, Error> {
///     // Build response...
///     LegacyResponse::new(builder)
///         .with_status(200)
///         .with_header("Content-Type", "application/octet-stream")
/// }
/// ```
///
/// The generic parameter T documents the FlatBuffer response type.
#[deprecated(since = "0.2.0", note = "Use the new Response<T> struct instead")]
pub struct LegacyResponse<T> {
    builder: FlatBufferBuilder<'static>,
    status_code: u16,
    headers: HashMap<String, String>,
    _phantom: std::marker::PhantomData<T>,
}

#[allow(deprecated)]
impl<T> LegacyResponse<T> {
    /// Create a new Response from a FlatBufferBuilder that has been finalized
    ///
    /// The builder should have already called `finish()` on the response of type T.
    /// Default status code is 200 OK.
    ///
    /// # Example
    /// ```ignore
    /// let mut builder = FlatBufferBuilder::new();
    /// let response = PingResponse::create(&mut builder, &args);
    /// builder.finish(response, None);
    /// Response::<PingResponse>::new(builder)
    /// ```
    pub fn new(builder: FlatBufferBuilder<'static>) -> Self {
        Self {
            builder,
            status_code: 200,
            headers: HashMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a Response by providing a closure that builds the response
    ///
    /// This encapsulates all the builder boilerplate. The closure receives a mutable
    /// reference to a FlatBufferBuilder and should create and finish the response.
    ///
    /// # Example
    /// ```ignore
    /// Response::with_body(|builder| {
    ///     let msg = builder.create_string("Hello");
    ///     let response = PingResponse::create(builder, &PingResponseArgs {
    ///         message: Some(msg),
    ///         timestamp: 123,
    ///         success: true,
    ///     });
    ///     builder.finish(response, None);
    /// })
    /// ```
    pub fn with_body<F>(f: F) -> Self
    where
        F: FnOnce(&mut FlatBufferBuilder<'static>),
    {
        let mut builder = FlatBufferBuilder::new();
        f(&mut builder);
        Self::new(builder)
    }

    /// Set the HTTP status code for this response
    ///
    /// # Example
    /// ```ignore
    /// Response::new(builder).with_status(201)  // Created
    /// Response::new(builder).with_status(404)  // Not Found
    /// ```
    pub fn with_status(mut self, status: u16) -> Self {
        self.status_code = status;
        self
    }

    /// Add an HTTP header to this response
    ///
    /// # Example
    /// ```ignore
    /// Response::new(builder)
    ///     .with_header("Content-Type", "application/octet-stream")
    ///     .with_header("X-Request-ID", "123")
    /// ```
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Get the HTTP status code
    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    /// Get a reference to the headers
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// Get the serialized bytes from the response
    pub fn into_bytes(self) -> Vec<u8> {
        self.builder.finished_data().to_vec()
    }

    /// Get a reference to the serialized bytes
    pub fn as_bytes(&self) -> &[u8] {
        self.builder.finished_data()
    }

    /// Consume self and return (bytes, status_code, headers)
    pub fn into_parts(self) -> (Vec<u8>, u16, HashMap<String, String>) {
        let bytes = self.builder.finished_data().to_vec();
        (bytes, self.status_code, self.headers)
    }
}

/// Creates a new FlatBufferBuilder with the default capacity
pub fn new_builder() -> FlatBufferBuilder<'static> {
    FlatBufferBuilder::new()
}

/// Creates a new FlatBufferBuilder with a specific capacity
pub fn new_builder_with_capacity(capacity: usize) -> FlatBufferBuilder<'static> {
    FlatBufferBuilder::with_capacity(capacity)
}

/// Verifies a flatbuffer message
///
/// Returns true if the buffer is valid, false otherwise
pub fn verify_buffer<'a, T>(buf: &'a [u8]) -> bool
where
    T: flatbuffers::Follow<'a> + flatbuffers::Verifiable + 'a,
{
    flatbuffers::root::<T>(buf).is_ok()
}

/// Safely reads a root table from a buffer
///
/// This performs verification before returning the root
pub fn get_root<'a, T>(
    buf: &'a [u8],
) -> Result<<T as flatbuffers::Follow<'a>>::Inner, flatbuffers::InvalidFlatbuffer>
where
    T: flatbuffers::Follow<'a> + flatbuffers::Verifiable + 'a,
{
    flatbuffers::root::<T>(buf)
}

/// Handler function type for route registration
///
/// This represents the signature of handler functions that can be used with
/// the routing system. Handlers take raw bytes and content-type string,
/// and return a Result with response bytes and response content-type.
///
/// Supports both `application/json` and `application/x-flatbuffers` content types.
/// The response content-type matches the request content-type.
///
/// Note: The actual user-defined handlers return Result<Response, E>
/// where E can be converted to flatbed::Error. The macro wrapper handles
/// content-type negotiation and serialization/deserialization.
pub type HandlerFn = fn(Vec<u8>, &str) -> Result<(Vec<u8>, &'static str), Error>;

/// Async handler function type for hyper integration
///
/// Takes request parts, body bytes, content-type, application context, and
/// returns response parts. The context is passed as `Arc<dyn Any>` and
/// downcasted to the concrete type by the `#[route]` macro wrapper.
pub type AsyncHandlerFn = fn(
    RequestParts,
    Vec<u8>,
    &str,
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<ResponseParts, Error>> + Send>,
>;

/// HTTP method for route registration
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    /// Returns the method as an uppercase string
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Route information collected from #[route] decorators
#[derive(Clone, Copy)]
pub struct RouteInfo {
    pub path: &'static str,
    pub method: HttpMethod,
    pub request_type: &'static str,
    pub response_type: &'static str,
    pub handler: HandlerFn,
    pub async_handler: AsyncHandlerFn,
    pub openapi: OpenApiRouteInfo,
}

// Manual Debug implementation to avoid printing function pointers
impl std::fmt::Debug for RouteInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteInfo")
            .field("path", &self.path)
            .field("method", &self.method)
            .field("request_type", &self.request_type)
            .field("response_type", &self.response_type)
            .field("handler", &"<function>")
            .field("async_handler", &"<async_function>")
            .field("openapi", &self.openapi)
            .finish()
    }
}

inventory::collect!(RouteInfo);

// ============================================================================
// Worker Registration Types
// ============================================================================

/// Worker function signature for background tasks
///
/// Workers receive the application context wrapped as `Arc<dyn Any + Send + Sync>`.
/// The `#[worker]` macro generates a wrapper that downcasts to the correct concrete type.
pub type WorkerFn = fn(
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>,
>;

/// Drain function signature for graceful shutdown of workers.
///
/// Called during shutdown to allow workers to finish in-progress work.
/// Receives the application context and returns a future that completes
/// when draining is done.
pub type DrainFn = fn(
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>,
>;

/// Worker metadata registered via the `#[worker]` macro or `register_*!` macros
///
/// This struct is collected via the `inventory` crate at runtime.
#[derive(Clone, Copy)]
pub struct WorkerInfo {
    /// Worker name (from attribute or function name)
    pub name: &'static str,
    /// Optional description of what the worker does
    pub description: Option<&'static str>,
    /// The worker function that receives the context
    pub worker: WorkerFn,
}

// Manual Debug implementation to avoid printing function pointers
impl std::fmt::Debug for WorkerInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerInfo")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("worker", &"<worker_fn>")
            .finish()
    }
}

/// Drain function metadata registered alongside workers that support graceful shutdown.
///
/// Workers registered via `register_kube_reconciler!` may provide a drain function
/// that is called during graceful shutdown to finish in-progress work.
/// Collected separately from [`WorkerInfo`] to maintain backward compatibility
/// with existing `#[worker]` macro-generated code.
#[derive(Clone, Copy)]
pub struct WorkerDrainInfo {
    /// Worker name (must match the corresponding WorkerInfo name)
    pub name: &'static str,
    /// The drain function called during graceful shutdown
    pub drain: DrainFn,
}

inventory::collect!(WorkerDrainInfo);

/// Get all registered drain functions.
pub fn get_worker_drains() -> Vec<&'static WorkerDrainInfo> {
    inventory::iter::<WorkerDrainInfo>.into_iter().collect()
}

inventory::collect!(WorkerInfo);

/// Get all registered workers
///
/// Returns a vector of all workers registered via `#[worker]` macros.
///
/// # Example
///
/// ```rust,ignore
/// for worker in get_workers() {
///     println!("Worker: {} - {:?}", worker.name, worker.description);
/// }
/// ```
pub fn get_workers() -> Vec<&'static WorkerInfo> {
    inventory::iter::<WorkerInfo>.into_iter().collect()
}

// ============================================================================
// Worker Trait
// ============================================================================

/// Trait-based background worker.
///
/// Implement this trait to define a simple background worker without proc macros.
/// The runtime executor [`run_basic_worker`] handles context downcasting —
/// matching the behavior of `#[worker]`.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::{Worker, FlatbedWorkerError};
/// use std::sync::Arc;
///
/// struct HealthChecker;
///
/// impl Worker for HealthChecker {
///     type Context = AppContext;
///     const NAME: &'static str = "health-checker";
///     const DESCRIPTION: Option<&'static str> = Some("Periodically checks health");
///
///     fn run(
///         &self,
///         ctx: Arc<Self::Context>,
///     ) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
///         Box::pin(async move {
///             // worker logic
///             Ok(())
///         })
///     }
/// }
///
/// flatbed::register_worker!(HealthChecker, AppContext);
/// ```
pub trait Worker: Send + Sync + 'static {
    /// The application context type.
    type Context: Send + Sync + 'static;

    /// Worker name used for logging and identification.
    const NAME: &'static str;

    /// Optional description of what this worker does.
    const DESCRIPTION: Option<&'static str> = None;

    /// Run the worker. Called once after the server is ready.
    fn run(&self, ctx: std::sync::Arc<Self::Context>) -> BoxFuture<Result<(), FlatbedWorkerError>>;
}

/// Run a [`Worker`] by downcasting the context and delegating to `Worker::run`.
pub fn run_basic_worker<W, C>(
    ctx: std::sync::Arc<dyn std::any::Any + Send + Sync>,
) -> BoxFuture<Result<(), FlatbedWorkerError>>
where
    W: Worker<Context = C> + Default,
    C: Send + Sync + 'static,
{
    Box::pin(async move {
        let ctx: std::sync::Arc<C> = ctx
            .downcast::<C>()
            .unwrap_or_else(|_| panic!("worker '{}' context type mismatch", W::NAME));
        let worker = W::default();
        worker.run(ctx).await
    })
}

// ============================================================================
// Registration Macros
// ============================================================================

/// Register a [`KubeReconciler`] implementor with the Flatbed worker system.
///
/// This macro generates a `WorkerFn`-compatible wrapper that delegates to
/// [`run_kube_reconciler`] and submits a [`WorkerInfo`] via `inventory`.
///
/// # Usage
///
/// ```rust,ignore
/// struct MyReconciler;
/// impl flatbed::KubeReconciler for MyReconciler { /* ... */ }
///
/// flatbed::register_kube_reconciler!(MyReconciler, AppContext);
/// ```
#[cfg(all(feature = "nats", feature = "k8s"))]
#[macro_export]
macro_rules! register_kube_reconciler {
    ($reconciler:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$reconciler as $crate::k8s::KubeReconciler>::NAME,
                description: <$reconciler as $crate::k8s::KubeReconciler>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin($crate::k8s::run_kube_reconciler::<$reconciler, $context>(ctx))
                    }
                    __worker
                },
            }
        }
    };
}

/// Register a [`KubeNativeReconciler`] implementor with the Flatbed
/// worker system.
///
/// The generated wrapper delegates to [`run_kube_native_reconciler`]
/// and submits a [`WorkerInfo`] via `inventory`. Use this for
/// reconcilers whose contexts implement `HasKubeClient +
/// HasLeaderElection` only — no JetStream context bound required.
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(Default)]
/// struct MyReconciler;
/// impl flatbed::KubeNativeReconciler for MyReconciler { /* ... */ }
///
/// flatbed::register_kube_native_reconciler!(MyReconciler, AppContext);
/// ```
///
/// [`KubeNativeReconciler`]: crate::k8s::KubeNativeReconciler
/// [`run_kube_native_reconciler`]: crate::k8s::run_kube_native_reconciler
#[cfg(feature = "k8s")]
#[macro_export]
macro_rules! register_kube_native_reconciler {
    ($reconciler:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$reconciler as $crate::k8s::KubeNativeReconciler>::NAME,
                description: <$reconciler as $crate::k8s::KubeNativeReconciler>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin($crate::k8s::run_kube_native_reconciler::<$reconciler, $context>(ctx))
                    }
                    __worker
                },
            }
        }
    };
}

/// Register a [`KubeWatcher`] implementor with the Flatbed worker system.
///
/// Mirrors `register_kube_reconciler!` but uses the watcher-based executor
/// — the right primitive when the watched resource is owned by
/// kube-controller-manager (no finalizer possible) and Delete events
/// matter for correctness.
///
/// # Requirements
///
/// The implementor must also implement `Default` — the executor calls
/// `R::default()` once per executor lifetime. State that needs to
/// reset per-watch-burst should reset inside `on_init` (which fires
/// on every list+watch, including reopens after a transient error).
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(Default)]
/// struct EndpointWatcher { /* per-leader state behind Arc<Mutex<..>> */ }
/// impl flatbed::KubeWatcher for EndpointWatcher { /* ... */ }
///
/// flatbed::register_kube_watcher!(EndpointWatcher, AppContext);
/// ```
///
/// [`KubeWatcher`]: crate::k8s::KubeWatcher
#[cfg(feature = "k8s")]
#[macro_export]
macro_rules! register_kube_watcher {
    ($reconciler:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$reconciler as $crate::k8s::KubeWatcher>::NAME,
                description: <$reconciler as $crate::k8s::KubeWatcher>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin($crate::k8s::run_kube_watcher::<$reconciler, $context>(ctx))
                    }
                    __worker
                },
            }
        }
    };
}

/// Register a [`StreamWorker`] implementor with the Flatbed worker system.
///
/// This macro generates a `WorkerFn`-compatible wrapper that delegates to
/// [`run_stream_worker`] and submits a [`WorkerInfo`] via `inventory`.
///
/// # Usage
///
/// ```rust,ignore
/// struct MyWorker;
/// impl flatbed::StreamWorker for MyWorker { /* ... */ }
///
/// flatbed::register_stream_worker!(MyWorker, AppContext);
/// ```
#[cfg(feature = "nats")]
#[macro_export]
macro_rules! register_stream_worker {
    ($stream_worker:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$stream_worker as $crate::nats::StreamWorker>::NAME,
                description: <$stream_worker as $crate::nats::StreamWorker>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin($crate::nats::run_stream_worker::<$stream_worker, $context>(ctx))
                    }
                    __worker
                },
            }
        }
    };
}

/// Register a [`KvWorker`] implementor with the Flatbed worker system.
///
/// This macro generates a `WorkerFn`-compatible wrapper that delegates to
/// [`run_kv_worker`] and submits a [`WorkerInfo`] via `inventory`.
///
/// # Requirements
///
/// The implementor must also implement `Default` — the executor calls
/// `W::default()` once at spawn. `#[derive(Default)]` on the impl
/// struct is usually enough.
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(Default)]
/// struct CacheSubscriber;
/// impl flatbed::KvWorker for CacheSubscriber { /* ... */ }
///
/// flatbed::register_kv_worker!(CacheSubscriber, AppContext);
/// ```
///
/// [`KvWorker`]: crate::kv::KvWorker
/// [`run_kv_worker`]: crate::kv::run_kv_worker
#[cfg(feature = "nats")]
#[macro_export]
macro_rules! register_kv_worker {
    ($kv_worker:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$kv_worker as $crate::kv::KvWorker>::NAME,
                description: <$kv_worker as $crate::kv::KvWorker>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin($crate::kv::run_kv_worker::<$kv_worker, $context>(ctx))
                    }
                    __worker
                },
            }
        }
    };
}

/// Register a [`Worker`] implementor with the Flatbed worker system.
///
/// This macro generates a `WorkerFn`-compatible wrapper that delegates to
/// [`run_basic_worker`] and submits a [`WorkerInfo`] via `inventory`.
///
/// # Usage
///
/// ```rust,ignore
/// struct HealthChecker;
/// impl flatbed::Worker for HealthChecker { /* ... */ }
///
/// flatbed::register_worker!(HealthChecker, AppContext);
/// ```
#[macro_export]
macro_rules! register_worker {
    ($worker_type:ty, $context:ty) => {
        $crate::inventory::submit! {
            $crate::WorkerInfo {
                name: <$worker_type as $crate::Worker>::NAME,
                description: <$worker_type as $crate::Worker>::DESCRIPTION,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = Result<(), $crate::FlatbedWorkerError>> + Send>,
                    > {
                        $crate::run_basic_worker::<$worker_type, $context>(ctx)
                    }
                    __worker
                },
            }
        }
    };
}

/// Route map type: (path, method) -> RouteInfo with handler
pub type RouteMap = HashMap<(String, HttpMethod), RouteInfo>;

/// Validation error for route conflicts
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteConflict {
    pub path: String,
    pub method: HttpMethod,
    pub first_request: String,
    pub first_response: String,
    pub second_request: String,
    pub second_response: String,
}

impl std::fmt::Display for RouteConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Route conflict at {} '{}': ({}, {}) vs ({}, {})",
            self.method,
            self.path,
            self.first_request,
            self.first_response,
            self.second_request,
            self.second_response
        )
    }
}

impl std::error::Error for RouteConflict {}

/// Get the route map with all registered routes
///
/// Returns a HashMap mapping path strings to RouteInfo containing
/// handler function pointers and type information.
/// This is populated at runtime from all #[route] decorated functions.
///
/// # Example
/// ```rust,ignore
/// use flatbed::{route, get_routes};
///
/// #[route("/ping")]
/// fn handle_ping(req: Vec<u8>) -> Vec<u8> { ... }
///
/// let routes = get_routes();
/// let route_info = routes.get(&("/ping".to_string(), flatbed::HttpMethod::Post)).unwrap();
/// // Use route_info.handler to call the function
/// // Use route_info.request_type / response_type for metadata
/// ```
pub fn get_routes() -> &'static RouteMap {
    static ROUTES: LazyLock<RouteMap> = LazyLock::new(|| {
        let mut map = HashMap::new();
        for route in inventory::iter::<RouteInfo> {
            map.insert((route.path.to_string(), route.method), *route);
        }
        map
    });
    &ROUTES
}

/// Validate that all routes are unique (no duplicate paths)
///
/// Returns Ok(()) if all routes are valid, or Err with the first conflict found.
///
/// # Example
/// ```rust,ignore
/// use flatbed::validate_routes;
///
/// match validate_routes() {
///     Ok(()) => println!("All routes valid!"),
///     Err(conflict) => eprintln!("Route conflict: {}", conflict),
/// }
/// ```
#[allow(clippy::result_large_err)]
pub fn validate_routes() -> Result<(), RouteConflict> {
    let mut seen: HashMap<(String, HttpMethod), RouteInfo> = HashMap::new();

    for route in inventory::iter::<RouteInfo> {
        let key = (route.path.to_string(), route.method);

        if let Some(first_route) = seen.get(&key) {
            // Found a duplicate (path, method) with different types
            if first_route.request_type != route.request_type
                || first_route.response_type != route.response_type
            {
                return Err(RouteConflict {
                    path: route.path.to_string(),
                    method: route.method,
                    first_request: first_route.request_type.to_string(),
                    first_response: first_route.response_type.to_string(),
                    second_request: route.request_type.to_string(),
                    second_response: route.response_type.to_string(),
                });
            }
        }

        seen.insert(key, *route);
    }

    Ok(())
}

// OpenAPI generation (enabled with "openapi" feature)
#[cfg(feature = "openapi")]
mod openapi_generation {
    use super::{get_routes, FlatbedConfig, HttpMethod, RouteInfo, SchemaInfo};
    use std::collections::BTreeSet;
    use utoipa::openapi::{
        content::ContentBuilder,
        path::{OperationBuilder, PathItemBuilder},
        request_body::RequestBodyBuilder,
        response::ResponseBuilder,
        schema::{ObjectBuilder, Schema, SchemaFormat, SchemaType, Type},
        server::ServerBuilder,
        ComponentsBuilder, Deprecated, InfoBuilder, OpenApi, OpenApiBuilder, PathsBuilder, RefOr,
        Required,
    };

    /// Get all unique versions from registered routes
    pub fn get_route_versions() -> BTreeSet<String> {
        let mut versions = BTreeSet::new();
        for route in get_routes().values() {
            let version = route.openapi.version.unwrap_or("v1");
            versions.insert(version.to_string());
        }
        if versions.is_empty() {
            versions.insert("v1".to_string());
        }
        versions
    }

    /// Get the latest version (lexicographically highest)
    pub fn get_latest_version() -> String {
        get_route_versions()
            .into_iter()
            .next_back()
            .unwrap_or_else(|| "v1".to_string())
    }

    /// Generate FBS schema text from field info
    fn generate_fbs_schema_text(schema: &SchemaInfo) -> String {
        let mut fbs = format!("table {} {{\n", schema.name.trim_end_matches("Json"));
        for field in schema.fields {
            fbs.push_str(&format!("  {}: {};\n", field.name, field.fbs_type));
        }
        fbs.push('}');
        fbs
    }

    /// Build JSON schema from field info
    fn build_json_schema(schema: &SchemaInfo) -> Schema {
        let mut obj_builder = ObjectBuilder::new();

        let mut required_fields = Vec::new();
        for field in schema.fields {
            let field_schema: RefOr<Schema> = match field.field_type {
                "string" => RefOr::T(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .build(),
                )),
                "integer" => RefOr::T(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::Integer))
                        .build(),
                )),
                "number" => RefOr::T(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::Number))
                        .build(),
                )),
                "boolean" => RefOr::T(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::Boolean))
                        .build(),
                )),
                _ => RefOr::T(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(SchemaType::new(Type::String))
                        .build(),
                )),
            };
            obj_builder = obj_builder.property(field.name, field_schema);
            if field.required {
                required_fields.push(field.name.to_string());
            }
        }

        for required in required_fields {
            obj_builder = obj_builder.required(required);
        }

        Schema::Object(obj_builder.build())
    }

    /// Convert our HttpMethod to utoipa's HttpMethod
    fn to_utoipa_method(method: &HttpMethod) -> utoipa::openapi::path::HttpMethod {
        match method {
            HttpMethod::Get => utoipa::openapi::path::HttpMethod::Get,
            HttpMethod::Post => utoipa::openapi::path::HttpMethod::Post,
            HttpMethod::Put => utoipa::openapi::path::HttpMethod::Put,
            HttpMethod::Delete => utoipa::openapi::path::HttpMethod::Delete,
            HttpMethod::Patch => utoipa::openapi::path::HttpMethod::Patch,
            HttpMethod::Head => utoipa::openapi::path::HttpMethod::Head,
            HttpMethod::Options => utoipa::openapi::path::HttpMethod::Options,
        }
    }

    /// Build an OpenAPI operation from a route
    fn build_operation(
        route: &RouteInfo,
        components_builder: &mut ComponentsBuilder,
        schemas_added: &mut BTreeSet<String>,
    ) -> utoipa::openapi::path::Operation {
        // Build request body content
        let mut request_content = ContentBuilder::new();

        // Add JSON content type
        if let Some(req_schema) = &route.openapi.request_schema {
            let json_schema = build_json_schema(req_schema);
            request_content = request_content.schema(Some(json_schema));

            // Add schema to components if not already added
            if !schemas_added.contains(req_schema.name) {
                let component_schema = build_json_schema(req_schema);
                *components_builder =
                    std::mem::take(components_builder).schema(req_schema.name, component_schema);
                schemas_added.insert(req_schema.name.to_string());
            }
        }

        let request_body = RequestBodyBuilder::new()
            .content("application/json", request_content.build())
            .content(
                "application/x-flatbuffers",
                ContentBuilder::new()
                    .schema(Some(Schema::Object(
                        ObjectBuilder::new()
                            .schema_type(SchemaType::new(Type::String))
                            .format(Some(SchemaFormat::Custom("binary".to_string())))
                            .description(Some(format!(
                                "FlatBuffers binary data.\n\nSchema:\n```fbs\n{}\n```",
                                route
                                    .openapi
                                    .request_schema
                                    .as_ref()
                                    .map(generate_fbs_schema_text)
                                    .unwrap_or_default()
                            )))
                            .build(),
                    )))
                    .build(),
            )
            .required(Some(Required::True))
            .build();

        // Build response content
        let mut response_content = ContentBuilder::new();

        if let Some(resp_schema) = &route.openapi.response_schema {
            let json_schema = build_json_schema(resp_schema);
            response_content = response_content.schema(Some(json_schema));

            // Add schema to components if not already added
            if !schemas_added.contains(resp_schema.name) {
                let component_schema = build_json_schema(resp_schema);
                *components_builder =
                    std::mem::take(components_builder).schema(resp_schema.name, component_schema);
                schemas_added.insert(resp_schema.name.to_string());
            }
        }

        let response = ResponseBuilder::new()
            .description("Successful response")
            .content("application/json", response_content.build())
            .content(
                "application/x-flatbuffers",
                ContentBuilder::new()
                    .schema(Some(Schema::Object(
                        ObjectBuilder::new()
                            .schema_type(SchemaType::new(Type::String))
                            .format(Some(SchemaFormat::Custom("binary".to_string())))
                            .description(Some(format!(
                                "FlatBuffers binary data.\n\nSchema:\n```fbs\n{}\n```",
                                route
                                    .openapi
                                    .response_schema
                                    .as_ref()
                                    .map(generate_fbs_schema_text)
                                    .unwrap_or_default()
                            )))
                            .build(),
                    )))
                    .build(),
            )
            .build();

        // Build operation
        let mut operation_builder = OperationBuilder::new()
            .request_body(Some(request_body))
            .response("200", response);

        if let Some(tag) = route.openapi.tag {
            operation_builder = operation_builder.tag(tag);
        }
        if let Some(summary) = route.openapi.summary {
            operation_builder = operation_builder.summary(Some(summary.to_string()));
        }
        if let Some(operation_id) = route.openapi.operation_id {
            operation_builder = operation_builder.operation_id(Some(operation_id.to_string()));
        }
        if route.openapi.deprecated {
            operation_builder = operation_builder.deprecated(Some(Deprecated::True));
        }

        operation_builder.build()
    }

    /// Generate OpenAPI spec for a specific version
    pub fn generate_openapi_spec(config: &FlatbedConfig, version: &str) -> OpenApi {
        let mut paths_builder = PathsBuilder::new();
        let mut components_builder = ComponentsBuilder::new();
        let mut schemas_added: BTreeSet<String> = BTreeSet::new();

        // Filter routes by version and group by path
        let routes: Vec<&RouteInfo> = get_routes()
            .values()
            .filter(|r| r.openapi.version.unwrap_or("v1") == version)
            .collect();

        // Group routes by path so multiple methods on the same path share a PathItem
        let mut path_groups: std::collections::BTreeMap<&str, Vec<&RouteInfo>> =
            std::collections::BTreeMap::new();
        for route in &routes {
            path_groups.entry(route.path).or_default().push(route);
        }

        for (path, path_routes) in &path_groups {
            let mut path_item_builder = PathItemBuilder::new();
            for route in path_routes {
                let operation = build_operation(route, &mut components_builder, &mut schemas_added);
                let http_method = to_utoipa_method(&route.method);
                path_item_builder = path_item_builder.operation(http_method, operation);
            }
            paths_builder = paths_builder.path(*path, path_item_builder.build());
        }

        // Build info
        let mut info_builder = InfoBuilder::new().title(config.title).version(version);

        if let Some(desc) = config.description {
            info_builder = info_builder.description(Some(desc.to_string()));
        }

        let mut builder = OpenApiBuilder::new()
            .info(info_builder.build())
            .paths(paths_builder.build())
            .components(Some(components_builder.build()));

        // Add external URL if provided
        if let Some(ref url) = config.external_url {
            builder = builder.servers(Some(vec![ServerBuilder::new().url(url.clone()).build()]));
        }

        builder.build()
    }

    /// Generate OpenAPI spec as JSON string
    pub fn get_openapi_json_for_version(config: &FlatbedConfig, version: &str) -> String {
        let spec = generate_openapi_spec(config, version);
        serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(feature = "openapi")]
pub use openapi_generation::{
    generate_openapi_spec, get_latest_version, get_openapi_json_for_version, get_route_versions,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_builder() {
        let _builder = new_builder();
        // Builder created successfully
    }

    #[test]
    fn test_new_builder_with_capacity() {
        let _builder = new_builder_with_capacity(1024);
        // Builder with capacity created successfully
    }

    // ========================================================================
    // Request<T, C> tests
    // ========================================================================

    #[test]
    fn test_request_without_context() {
        let req: Request<String> = Request {
            body: "test body".to_string(),
            ctx: (),
            headers: HeaderMap::new(),
            method: Method::POST,
            path: "/test".to_string(),
            path_params: HashMap::new(),
            query_params: HashMap::new(),
            request_id: "req-123".to_string(),
        };

        assert_eq!(*req, "test body");
        assert_eq!(req.body, "test body");
        assert_eq!(req.path, "/test");
        assert_eq!(req.request_id(), "req-123");
    }

    #[test]
    fn test_request_with_context() {
        #[derive(Debug)]
        struct AppCtx {
            app_name: String,
        }

        let ctx = AppCtx {
            app_name: "test-app".to_string(),
        };

        let req: Request<String, AppCtx> = Request {
            body: "test body".to_string(),
            ctx,
            headers: HeaderMap::new(),
            method: Method::POST,
            path: "/test".to_string(),
            path_params: HashMap::new(),
            query_params: HashMap::new(),
            request_id: "req-456".to_string(),
        };

        assert_eq!(req.ctx.app_name, "test-app");
        assert_eq!(*req, "test body");
    }

    #[test]
    fn test_request_params_and_queries() {
        let mut path_params = HashMap::new();
        path_params.insert("id".to_string(), "123".to_string());

        let mut query_params = HashMap::new();
        query_params.insert("filter".to_string(), "active".to_string());

        let req: Request<String> = Request {
            body: "test".to_string(),
            ctx: (),
            headers: HeaderMap::new(),
            method: Method::POST,
            path: "/users/123".to_string(),
            path_params,
            query_params,
            request_id: "req-789".to_string(),
        };

        assert_eq!(req.param("id"), Some("123"));
        assert_eq!(req.param("nonexistent"), None);
        assert_eq!(req.query("filter"), Some("active"));
        assert_eq!(req.query("nonexistent"), None);
    }

    #[test]
    fn test_request_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", HeaderValue::from_static("custom-value"));

        let req: Request<String> = Request {
            body: "test".to_string(),
            ctx: (),
            headers,
            method: Method::POST,
            path: "/test".to_string(),
            path_params: HashMap::new(),
            query_params: HashMap::new(),
            request_id: "req-abc".to_string(),
        };

        assert_eq!(req.header("x-custom"), Some("custom-value"));
        assert_eq!(req.header("nonexistent"), None);
    }

    // ========================================================================
    // Response<T> tests
    // ========================================================================

    #[test]
    fn test_response_ok() {
        let resp = Response::ok("success".to_string());
        assert_eq!(resp.body, "success");
        assert_eq!(resp.status, StatusCode::OK);
        assert!(resp.headers.is_empty());
    }

    #[test]
    fn test_response_created() {
        let resp = Response::created("new resource".to_string());
        assert_eq!(resp.status, StatusCode::CREATED);
    }

    #[test]
    fn test_response_with_status() {
        let resp = Response::with_status("accepted".to_string(), StatusCode::ACCEPTED);
        assert_eq!(resp.status, StatusCode::ACCEPTED);
    }

    #[test]
    fn test_response_builder_pattern() {
        let resp = Response::ok("test".to_string())
            .status(StatusCode::CREATED)
            .header("x-custom", "value");

        assert_eq!(resp.status, StatusCode::CREATED);
        assert_eq!(
            resp.headers.get("x-custom").and_then(|v| v.to_str().ok()),
            Some("value")
        );
    }

    // ========================================================================
    // FlatbedWorkerError tests
    // ========================================================================

    #[test]
    fn test_flatbed_worker_error_new() {
        let err = FlatbedWorkerError::new("TASK_ERROR", "task failed");
        assert_eq!(err.code, "TASK_ERROR");
        assert_eq!(err.message, "task failed");
    }

    #[test]
    fn test_flatbed_worker_error_display() {
        let err = FlatbedWorkerError::new("CACHE_ERROR", "cache warmup failed");
        assert_eq!(format!("{}", err), "[CACHE_ERROR] cache warmup failed");
    }

    // ========================================================================
    // FlatbedRouteError<D> tests
    // ========================================================================

    #[test]
    fn test_flatbed_route_error_bad_request() {
        let err = FlatbedRouteError::bad_request("invalid input");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.message, "invalid input");
        assert!(err.code.is_empty());
        assert!(err.details.is_none());
    }

    #[test]
    fn test_flatbed_route_error_not_found() {
        let err = FlatbedRouteError::not_found("resource not found");
        assert_eq!(err.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_flatbed_route_error_internal() {
        let err = FlatbedRouteError::internal("something went wrong");
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_flatbed_route_error_with_code() {
        let err = FlatbedRouteError::bad_request("validation failed").code("VALIDATION_ERROR");
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn test_flatbed_route_error_with_details() {
        #[derive(Debug, Clone)]
        struct ErrorDetails {
            field: String,
        }

        let details = ErrorDetails {
            field: "email".to_string(),
        };

        let err = FlatbedRouteError::bad_request("validation failed").with_details(details);
        assert!(err.details.is_some());
        assert_eq!(err.details.as_ref().unwrap().field, "email");
    }

    #[test]
    fn test_flatbed_route_error_display() {
        let err1 = FlatbedRouteError::bad_request("simple error");
        assert_eq!(format!("{}", err1), "simple error");

        let err2 = FlatbedRouteError::bad_request("coded error").code("ERR_001");
        assert_eq!(format!("{}", err2), "[ERR_001] coded error");
    }

    #[test]
    fn test_flatbed_route_error_with_header() {
        let err =
            FlatbedRouteError::unauthorized("auth required").header("www-authenticate", "Bearer");
        assert!(err.headers.contains_key("www-authenticate"));
    }

    #[test]
    #[allow(deprecated)]
    fn test_flatbed_error_deprecated_alias() {
        // Test that the deprecated alias still works
        let err: FlatbedError = FlatbedError::bad_request("test");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    // ========================================================================
    // RequestParts tests
    // ========================================================================

    #[test]
    fn test_request_parts_new() {
        let parts = RequestParts::new(Method::POST, "/api/test".to_string());
        assert_eq!(parts.method, Method::POST);
        assert_eq!(parts.path, "/api/test");
        assert!(!parts.request_id.is_empty()); // UUID generated
    }

    #[test]
    fn test_request_parts_request_id_from_header() {
        let mut parts = RequestParts::new(Method::POST, "/test".to_string());
        parts
            .headers
            .insert("x-request-id", HeaderValue::from_static("custom-id-123"));

        let parts = parts.with_request_id_from_header();
        assert_eq!(parts.request_id, "custom-id-123");
    }

    // ========================================================================
    // ResponseParts tests
    // ========================================================================

    #[test]
    fn test_response_parts_ok() {
        let parts = ResponseParts::ok(vec![1, 2, 3], "application/json");
        assert_eq!(parts.body, vec![1, 2, 3]);
        assert_eq!(parts.status, StatusCode::OK);
        assert_eq!(parts.content_type, "application/json");
    }

    #[test]
    fn test_response_parts_with_request_id() {
        let parts = ResponseParts::ok(vec![], "application/json").with_request_id("req-123");
        assert_eq!(
            parts
                .headers
                .get("x-request-id")
                .and_then(|v| v.to_str().ok()),
            Some("req-123")
        );
    }

    // ========================================================================
    // FlatbedConfig tests
    // ========================================================================

    #[test]
    fn test_flatbed_config_new() {
        let config = FlatbedConfig::new("Test API");
        assert_eq!(config.title, "Test API");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert!(config.description.is_none());
        assert!(config.external_url.is_none());
    }

    #[test]
    fn test_flatbed_config_default() {
        let config = FlatbedConfig::default();
        assert_eq!(config.title, "API");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn test_flatbed_config_builder_pattern() {
        let config = FlatbedConfig::new("My API")
            .host("127.0.0.1")
            .port(3000)
            .description("A test API")
            .external_url("https://api.example.com");

        assert_eq!(config.title, "My API");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3000);
        assert_eq!(config.description, Some("A test API"));
        assert_eq!(
            config.external_url,
            Some("https://api.example.com".to_string())
        );
    }

    #[test]
    fn test_flatbed_config_bind_address() {
        let config = FlatbedConfig::new("API").host("0.0.0.0").port(8080);
        assert_eq!(config.bind_address(), "0.0.0.0:8080");

        let config2 = FlatbedConfig::new("API").host("127.0.0.1").port(3000);
        assert_eq!(config2.bind_address(), "127.0.0.1:3000");
    }

    #[test]
    fn test_flatbed_config_debug() {
        let config = FlatbedConfig::new("Test API").host("localhost").port(9000);
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("Test API"));
        assert!(debug_str.contains("localhost"));
        assert!(debug_str.contains("9000"));
    }

    // ========================================================================
    // Worker trait tests
    // ========================================================================

    /// Test context for Worker trait tests.
    struct TestWorkerCtx {
        pub label: String,
    }

    /// A Worker implementation that records whether it was called.
    struct RecordingWorker;

    impl Default for RecordingWorker {
        fn default() -> Self {
            RecordingWorker
        }
    }

    impl Worker for RecordingWorker {
        type Context = TestWorkerCtx;
        const NAME: &'static str = "recording-worker";
        const DESCRIPTION: Option<&'static str> = Some("Records calls for testing");

        fn run(
            &self,
            ctx: std::sync::Arc<Self::Context>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>,
        > {
            Box::pin(async move {
                if ctx.label == "fail" {
                    return Err(FlatbedWorkerError::new("TEST", "intentional failure"));
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn run_basic_worker_downcasts_and_runs() {
        let ctx = std::sync::Arc::new(TestWorkerCtx {
            label: "success".to_string(),
        });
        let dyn_ctx: std::sync::Arc<dyn std::any::Any + Send + Sync> = ctx;
        let result = run_basic_worker::<RecordingWorker, TestWorkerCtx>(dyn_ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_basic_worker_propagates_error() {
        let ctx = std::sync::Arc::new(TestWorkerCtx {
            label: "fail".to_string(),
        });
        let dyn_ctx: std::sync::Arc<dyn std::any::Any + Send + Sync> = ctx;
        let result = run_basic_worker::<RecordingWorker, TestWorkerCtx>(dyn_ctx).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "TEST");
    }

    #[tokio::test]
    async fn run_basic_worker_panics_on_context_type_mismatch() {
        // Provide the wrong context type (String instead of TestWorkerCtx)
        let wrong_ctx: std::sync::Arc<dyn std::any::Any + Send + Sync> =
            std::sync::Arc::new("wrong type".to_string());

        let result = std::panic::AssertUnwindSafe(
            run_basic_worker::<RecordingWorker, TestWorkerCtx>(wrong_ctx),
        );
        let panic_result = futures::FutureExt::catch_unwind(result).await;
        assert!(
            panic_result.is_err(),
            "run_basic_worker should panic on context type mismatch"
        );
    }

    // ========================================================================
    // Worker trait defaults
    // ========================================================================

    #[test]
    fn worker_description_default_is_some_when_set() {
        assert_eq!(
            <RecordingWorker as Worker>::DESCRIPTION,
            Some("Records calls for testing")
        );
    }

    /// A worker with no description override to test the default.
    struct MinimalWorker;

    impl Default for MinimalWorker {
        fn default() -> Self {
            MinimalWorker
        }
    }

    impl Worker for MinimalWorker {
        type Context = TestWorkerCtx;
        const NAME: &'static str = "minimal-worker";

        fn run(
            &self,
            _ctx: std::sync::Arc<Self::Context>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>,
        > {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn worker_description_default_is_none() {
        assert!(<MinimalWorker as Worker>::DESCRIPTION.is_none());
    }

    // ========================================================================
    // register_worker! macro tests
    // ========================================================================

    // Register MinimalWorker via the macro so inventory picks it up.
    register_worker!(MinimalWorker, TestWorkerCtx);

    #[test]
    fn register_worker_macro_makes_worker_discoverable() {
        let workers = get_workers();
        let found = workers
            .iter()
            .find(|w| w.name == <MinimalWorker as Worker>::NAME);
        assert!(
            found.is_some(),
            "MinimalWorker should be discoverable via inventory"
        );
    }

    #[test]
    fn registered_worker_name_matches_trait_constant() {
        let workers = get_workers();
        let found = workers.iter().find(|w| w.name == "minimal-worker");
        assert!(found.is_some());
        let info = found.unwrap();
        assert_eq!(info.name, <MinimalWorker as Worker>::NAME);
        assert_eq!(info.description, <MinimalWorker as Worker>::DESCRIPTION);
    }

    #[tokio::test]
    async fn registered_worker_can_be_invoked_via_inventory() {
        let workers = get_workers();
        let info = workers
            .iter()
            .find(|w| w.name == "minimal-worker")
            .expect("MinimalWorker should be registered");

        let ctx = std::sync::Arc::new(TestWorkerCtx {
            label: "inventory-test".to_string(),
        });
        let dyn_ctx: std::sync::Arc<dyn std::any::Any + Send + Sync> = ctx;
        let result = (info.worker)(dyn_ctx).await;
        assert!(result.is_ok());
    }
}
