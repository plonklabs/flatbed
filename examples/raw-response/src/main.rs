//! Returning arbitrary media from a handler with `Response::raw`.
//!
//! The typed path serializes a struct to JSON or FlatBuffer. `Response::raw`
//! is the escape hatch: it emits bytes verbatim under any `Content-Type`, for
//! bodies that path can't express — CSV, SVG, an image, a rendered page.

mod generated {
    #![allow(warnings, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/report_flatbed.rs"));
}
use generated::report::{BadgeRequest, CsvRequest};

use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};

/// A CSV report built at request time, returned as `text/csv`.
#[route("/report.csv", method = "POST", tag = "Raw", summary = "CSV report")]
async fn report_csv(req: Request<CsvRequest>) -> Result<Response<()>, FlatbedRouteError> {
    let label = req.body.label.as_deref().unwrap_or("row");
    let mut csv = String::from("index,label\n");
    for i in 1..=req.body.count.max(0) {
        csv.push_str(&format!("{i},{label}\n"));
    }
    Ok(Response::raw(csv.into_bytes(), "text/csv; charset=utf-8"))
}

/// A tiny SVG badge, returned as `image/svg+xml` — the same primitive, a
/// different media type.
#[route("/badge.svg", method = "POST", tag = "Raw", summary = "SVG badge")]
async fn badge_svg(req: Request<BadgeRequest>) -> Result<Response<()>, FlatbedRouteError> {
    let label = req.body.label.as_deref().unwrap_or("flatbed");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="20">
  <rect width="120" height="20" fill="#4c1"/>
  <text x="60" y="14" fill="#fff" text-anchor="middle" font-family="sans-serif" font-size="11">{label}</text>
</svg>"##
    );
    Ok(Response::raw(svg.into_bytes(), "image/svg+xml"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = FlatbedConfig::new("raw-response")
        .host("0.0.0.0")
        .port(8080)
        .splash("raw-response — POST /report.csv, POST /badge.svg");
    Flatbed::run(config, |_| async { Ok(()) }).await?;
    Ok(())
}
