//! flatbed with the `openapi` feature: routes are tagged with `tag` / `summary`
//! / `version`, and the framework generates an OpenAPI 3 document from them.
//!
//! The server exposes:
//! - `GET /openapi.json`           — spec for the latest API version
//! - `GET /v{version}/openapi.json` — a specific version

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/api_flatbed.rs"));
}
use generated::api::{EchoRequest, EchoResponse, GreetRequest, GreetResponse};

use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};

#[route(
    "/greet",
    method = "POST",
    version = "v1",
    tag = "Greetings",
    summary = "Greet someone by name"
)]
async fn greet(req: Request<GreetRequest>) -> Result<Response<GreetResponse>, FlatbedRouteError> {
    let name = req.body.name.as_deref().unwrap_or("world");
    Ok(Response::ok(GreetResponse {
        greeting: Some(format!("Hello, {name}!")),
    }))
}

#[route(
    "/echo",
    method = "POST",
    version = "v1",
    tag = "Utility",
    summary = "Repeat a message N times"
)]
async fn echo(req: Request<EchoRequest>) -> Result<Response<EchoResponse>, FlatbedRouteError> {
    let times = req.body.times.max(1) as usize;
    let message = req.body.message.as_deref().unwrap_or("");
    Ok(Response::ok(EchoResponse {
        message: Some(vec![message; times].join(" ")),
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("openapi-example")
        .description("A flatbed service that publishes its own OpenAPI spec")
        .host("0.0.0.0")
        .port(8080)
        .splash("openapi-example — see GET /openapi.json");
    Flatbed::run(config, |_| async { Ok(()) }).await?;
    Ok(())
}
