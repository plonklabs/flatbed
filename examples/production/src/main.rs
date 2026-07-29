//! A deployable flatbed service — a business route, telemetry (so a deployment's
//! readiness probe passes), and a served spec + reflection to generate a typed
//! client from, all on one listener:
//!
//! - `POST /greet`       — the business route (typed request/response)
//! - `GET /openapi.json` — generated OpenAPI 3 spec (`openapi` feature)
//! - `GET /schema.bfbs`  — FlatBuffer reflection, so a client can codegen from
//!   the running server
//! - `GET /healthz` `/readyz` `/metrics` — telemetry (`prometheus` feature); the
//!   readiness probe is what lets a Kubernetes/Plonk deployment receive traffic
//!
//! `/metrics` reports a `greets_total` counter bumped on every `/greet`.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/greet_flatbed.rs"));
}
use generated::greet::{GreetRequest, GreetResponse};

use std::sync::Arc;

use flatbed::telemetry::prometheus::PrometheusTelemetryService;
use flatbed::telemetry::Counter;
use flatbed::{
    route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response, TelemetryConfig,
    TelemetryService,
};

#[derive(Clone)]
struct AppContext {
    greets: Arc<dyn Counter<u64>>,
}

#[route(
    "/greet",
    method = "POST",
    version = "v1",
    tag = "Greetings",
    summary = "Greet someone by name"
)]
async fn greet(
    req: Request<GreetRequest, Arc<AppContext>>,
) -> Result<Response<GreetResponse>, FlatbedRouteError> {
    req.ctx.greets.inc();
    let name = req.body.name.as_deref().unwrap_or("world");
    Ok(Response::ok(GreetResponse {
        greeting: Some(format!("Hello, {name}!")),
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let tel_config = TelemetryConfig::new(
        Some("production-example".to_string()),
        Some("0.0.0.0".to_string()),
        Some(8080),
    )?;
    let telemetry = PrometheusTelemetryService::new(tel_config);
    let greets = telemetry.register_u64_counter("greets_total", "Total greetings served", None)?;

    let config = FlatbedConfig::new("production-example")
        .description("A deployable flatbed service: an endpoint, telemetry, and a served spec")
        .host("0.0.0.0")
        .port(8080)
        .with_telemetry(telemetry);

    Flatbed::run(config, move |_| {
        let greets = greets.clone();
        async move { Ok(AppContext { greets }) }
    })
    .await?;
    Ok(())
}
