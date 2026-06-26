//! Hyper integration for flatbed HTTP server
//!
//! This module provides a pure hyper-based HTTP server implementation,
//! replacing the actix-web integration. It supports:
//!
//! - HTTP/1.1 and HTTP/2 cleartext (no TLS - Envoy handles it)
//! - Path-based routing with parameter extraction
//! - Content-type negotiation (JSON and FlatBuffer)
//! - Built-in telemetry endpoints (/healthz, /readyz, /metrics)
//! - OpenAPI spec endpoint (/openapi.json)
//!
//! # Architecture
//!
//! Flatbed is designed to run inside Kubernetes pods with an Envoy sidecar proxy.
//! Envoy handles TLS termination, mTLS, and external routing. Flatbed only supports
//! cleartext HTTP since all traffic is pod-internal.
//!
//! # Example
//!
//! ```rust,ignore
//! use flatbed::{route, Request, Response, FlatbedError};
//!
//! #[flatbed::main(bind = "0.0.0.0:8080")]
//! async fn main() {
//!     println!("Server started");
//! }
//! ```

mod endpoints;
mod router;
mod server;
mod service;
mod shutdown;

pub use endpoints::*;
pub use router::{HandlerFn, RouteEntry, Router};
pub use server::AutoServer;
pub use service::{FlatbedService, ServiceContext};
pub use shutdown::{shutdown_signal, ShutdownController};

use crate::{get_routes, RouteInfo};

/// Build a router from inventory-registered routes
///
/// This function creates a Router populated with all routes registered
/// via the `#[route]` macro. Uses the async_handler from each RouteInfo.
pub fn build_router() -> Router {
    let mut router = Router::default();

    // Add user-defined routes from inventory
    for ((path, _method), route_info) in get_routes().iter() {
        let entry = RouteEntry {
            info: *route_info,
            handler: route_info.async_handler,
        };
        router.add_route(path, route_info.method.as_str(), entry);
    }

    router
}

/// Build a router with a custom handler factory
///
/// This allows overriding the default async_handler for each route.
pub fn build_router_with<F>(make_handler: F) -> Router
where
    F: Fn(&RouteInfo) -> HandlerFn,
{
    let mut router = Router::default();

    // Add user-defined routes from inventory
    for ((path, _method), route_info) in get_routes().iter() {
        let handler = make_handler(route_info);
        let entry = RouteEntry {
            info: *route_info,
            handler,
        };
        router.add_route(path, route_info.method.as_str(), entry);
    }

    router
}
