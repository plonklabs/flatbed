//! Application context + a background worker.
//!
//! - The boot closure builds an `AppContext` once; the framework stores it
//!   before signaling ready, so both handlers and workers see the same value.
//! - A route reads from the context via `Request<T, Arc<AppContext>>`.
//! - A `Worker` runs in the background (spawned only after boot completes) and
//!   logs a heartbeat that includes the shared context, proving it has access.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/info_flatbed.rs"));
}
use generated::info::{InfoRequest, InfoResponse};

use std::sync::Arc;
use std::time::Duration;

use flatbed::{
    route, BoxFuture, Flatbed, FlatbedConfig, FlatbedRouteError, FlatbedWorkerError, Request,
    Response, Worker,
};

#[derive(Clone)]
struct AppContext {
    /// A stand-in for real boot state (a DB pool, a client, …). Set once in the
    /// boot closure and shared by every handler and worker.
    started_at: String,
}

#[route(
    "/info",
    method = "POST",
    tag = "Info",
    summary = "Greet using shared context"
)]
async fn info(
    req: Request<InfoRequest, Arc<AppContext>>,
) -> Result<Response<InfoResponse>, FlatbedRouteError> {
    let name = req.body.name.as_deref().unwrap_or("world");
    Ok(Response::ok(InfoResponse {
        greeting: Some(format!("Hello, {name}!")),
        started_at: Some(req.ctx.started_at.clone()),
    }))
}

/// Background worker: logs a heartbeat every few seconds, reading the same
/// shared context the handlers use.
#[derive(Default)]
struct Heartbeat;

impl Worker for Heartbeat {
    type Context = AppContext;
    const NAME: &'static str = "heartbeat";
    const DESCRIPTION: Option<&'static str> = Some("logs a heartbeat every few seconds");

    fn run(&self, ctx: Arc<Self::Context>) -> BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async move {
            loop {
                println!("[heartbeat] alive — context started_at={}", ctx.started_at);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        })
    }
}

flatbed::register_worker!(Heartbeat, AppContext);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("context-worker-example")
        .host("0.0.0.0")
        .port(8080)
        .splash("context-worker-example — POST /info");

    Flatbed::run(config, |_| async {
        // Real services would connect a DB / build clients here. We just stamp a
        // value to make the shared context observable.
        let started_at = format!("boot-{}", std::process::id());
        Ok(AppContext { started_at })
    })
    .await?;
    Ok(())
}
