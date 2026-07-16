//! A flatbed service that serves a JSON API and a bundled SPA from one origin.
//!
//! `/api/*` is handled by declared `#[route]`s; every other GET is served from
//! `dist/` by the `static_route!` mount, with unknown paths falling back to
//! `index.html` so client-side routing works. One origin, no CORS.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/api_flatbed.rs"));
}
use generated::api::{HelloRequest, HelloResponse};

use flatbed::{route, static_route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};

#[route("/api/hello", method = "POST", tag = "Api", summary = "Greet a caller")]
async fn hello(req: Request<HelloRequest>) -> Result<Response<HelloResponse>, FlatbedRouteError> {
    let name = req.body.name.as_deref().unwrap_or("world");
    Ok(Response::ok(HelloResponse {
        message: Some(format!("hello, {name}")),
    }))
}

// Serve the built SPA from dist/. The declared /api/* route above wins; unknown
// non-API GETs fall back to index.html so client-side routes load the app shell.
// `dir` is resolved relative to the process working directory (the Dockerfile
// sets WORKDIR /app and copies dist there).
static_route!(mount = "/", dir = "dist", fallback = "index.html");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("spa").host("0.0.0.0").port(8080);
    Flatbed::run(config, |_| async { Ok(()) }).await?;
    Ok(())
}
