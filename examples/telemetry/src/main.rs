//! flatbed with the `telemetry` + `prometheus` features.
//!
//! Enabling telemetry auto-registers three operational endpoints:
//! - `GET /healthz` — liveness probe ("OK")
//! - `GET /readyz`  — readiness probe ("Ready" once boot completes)
//! - `GET /metrics` — Prometheus metrics
//!
//! The application context carries a counter that the `/ping` handler bumps, so
//! `/metrics` reflects real traffic.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/ping_flatbed.rs"));
}
use generated::ping::{PingRequest, PingResponse};

use std::sync::Arc;

use flatbed::telemetry::prometheus::PrometheusTelemetryService;
use flatbed::telemetry::Counter;
use flatbed::{
    route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response, TelemetryConfig,
    TelemetryService,
};

#[derive(Clone)]
struct AppContext {
    ping_requests: Arc<dyn Counter<u64>>,
}

#[route("/ping", method = "POST", tag = "Health", summary = "Ping")]
async fn ping(
    req: Request<PingRequest, Arc<AppContext>>,
) -> Result<Response<PingResponse>, FlatbedRouteError> {
    req.ctx.ping_requests.inc();
    let msg = req.body.message.as_deref().unwrap_or("");
    Ok(Response::ok(PingResponse {
        message: Some(format!("pong: {msg}")),
        success: true,
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let tel_config = TelemetryConfig::new(
        Some("telemetry-example".to_string()),
        Some("0.0.0.0".to_string()),
        Some(8080),
    )?;
    let telemetry = PrometheusTelemetryService::new(tel_config);
    let ping_requests =
        telemetry.register_u64_counter("ping_requests_total", "Total ping requests", None)?;

    let config = FlatbedConfig::new("telemetry-example")
        .host("0.0.0.0")
        .port(8080)
        .with_telemetry(telemetry);

    Flatbed::run(config, move |_| {
        let ping_requests = ping_requests.clone();
        async move { Ok(AppContext { ping_requests }) }
    })
    .await?;
    Ok(())
}
