//! Smallest possible flatbed service: one route, no context, no extra features.
//!
//! The same `/ping` handler serves both JSON and binary FlatBuffer clients —
//! flatbed picks the codec from the request's `Content-Type` and mirrors it on
//! the response.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/ping_flatbed.rs"));
}
use generated::ping::{PingRequest, PingResponse};

use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};

#[route("/ping", method = "POST", tag = "Health", summary = "Ping")]
async fn ping(req: Request<PingRequest>) -> Result<Response<PingResponse>, FlatbedRouteError> {
    let msg = req.body.message.as_deref().unwrap_or("");
    Ok(Response::ok(PingResponse {
        message: Some(format!("pong: {msg}")),
        success: true,
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("minimal-ping")
        .host("0.0.0.0")
        .port(8080)
        .splash("minimal-ping — POST /ping");
    Flatbed::run(config, |_| async { Ok(()) }).await?;
    Ok(())
}
