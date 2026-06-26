//! Built-in endpoints for telemetry and OpenAPI
//!
//! These endpoints are automatically registered when the corresponding features are enabled.

use crate::{ResponseParts, StatusCode};

#[cfg(feature = "openapi")]
use crate::FlatbedConfig;

#[cfg(feature = "openapi")]
use crate::{get_latest_version, get_openapi_json_for_version, get_route_versions};

/// Build a health check response
pub fn health_response() -> ResponseParts {
    ResponseParts {
        body: b"OK".to_vec(),
        status: StatusCode::OK,
        headers: crate::HeaderMap::new(),
        content_type: "text/plain",
    }
}

/// Build a readiness response
pub fn ready_response() -> ResponseParts {
    ResponseParts {
        body: b"Ready".to_vec(),
        status: StatusCode::OK,
        headers: crate::HeaderMap::new(),
        content_type: "text/plain",
    }
}

/// Build a metrics response
#[cfg(feature = "telemetry")]
pub fn metrics_response(metrics: String) -> ResponseParts {
    ResponseParts {
        body: metrics.into_bytes(),
        status: StatusCode::OK,
        headers: crate::HeaderMap::new(),
        content_type: "text/plain; version=0.0.4; charset=utf-8",
    }
}

/// Build an OpenAPI spec response
#[cfg(feature = "openapi")]
pub fn openapi_response(config: &FlatbedConfig, version: Option<&str>) -> ResponseParts {
    let latest = get_latest_version();
    let version = version.unwrap_or(&latest);
    let json = get_openapi_json_for_version(config, version);

    ResponseParts {
        body: json.into_bytes(),
        status: StatusCode::OK,
        headers: crate::HeaderMap::new(),
        content_type: "application/json",
    }
}

/// Check if a path is a telemetry endpoint
#[cfg(feature = "telemetry")]
pub fn is_telemetry_endpoint(path: &str) -> bool {
    matches!(path, "/healthz" | "/readyz" | "/metrics")
}

/// Check if a path is an OpenAPI endpoint
#[cfg(feature = "openapi")]
pub fn is_openapi_endpoint(path: &str) -> bool {
    if path == "/openapi.json" {
        return true;
    }

    // Check versioned endpoints
    for version in get_route_versions() {
        if path == format!("/{}/openapi.json", version) {
            return true;
        }
    }

    false
}

/// Extract version from OpenAPI endpoint path
#[cfg(feature = "openapi")]
pub fn extract_openapi_version(path: &str) -> Option<String> {
    if path == "/openapi.json" {
        return Some(get_latest_version());
    }

    // Match /v{version}/openapi.json
    get_route_versions()
        .into_iter()
        .find(|version| path == format!("/{}/openapi.json", version))
}
