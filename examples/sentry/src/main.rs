//! flatbed reporting its server failures to Sentry, or to any server speaking
//! Sentry's protocol (GlitchTip, Bugsink), with the `sentry` feature.
//!
//! - `GET /fail`  — answers `500 UPSTREAM_DOWN`, reported as one event
//! - `GET /panic` — panics; flatbed answers `500 PANIC` and reports the panic
//!   with its backtrace
//!
//! Without `SENTRY_DSN` the reporter is a no-op and the service runs as usual.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/sentry_flatbed.rs"));
}
use generated::sentry_example::Empty;

use std::sync::Arc;

use flatbed::sentry::SentryReporter;
use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[route("/fail", method = "GET", tag = "Errors", summary = "Answer with a 500")]
async fn fail(_req: Request<Empty>) -> Result<Response<()>, FlatbedRouteError> {
    tracing::warn!("inventory service timed out");
    Err(FlatbedRouteError::internal("inventory service unavailable").code("UPSTREAM_DOWN"))
}

#[route("/panic", method = "GET", tag = "Errors", summary = "Panic in the handler")]
async fn panic(_req: Request<Empty>) -> Result<Response<()>, FlatbedRouteError> {
    let stock: Vec<u32> = Vec::new();
    Ok(Response::raw(stock[3].to_string().into_bytes(), "text/plain"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reporter = Arc::new(SentryReporter::from_env());
    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer())
        .with(reporter.layer())
        .init();

    let config = FlatbedConfig::new("sentry-example")
        .host("0.0.0.0")
        .port(8080)
        .splash("sentry-example — GET /fail, GET /panic")
        .with_error_reporter(reporter);
    Flatbed::run(config, |_| async { Ok(()) }).await?;
    Ok(())
}
