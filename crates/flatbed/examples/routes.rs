/// Example demonstrating the route decorator system with async handlers
///
/// This example shows:
/// - Defining async handlers with Request<T> and Response<T>
/// - Different HTTP methods (GET, POST with explicit method, POST as default)
/// - Route registration and discovery
/// - Running with hyper

// Generated test types (plain structs as primary API)
#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;
use generated::test::{TestRequest, TestResponse};

use flatbed::{get_routes, route, validate_routes, FlatbedRouteError, Request, Response};

// ============================================================================
// Example async handlers using Request<T> and Response<T>
// ============================================================================

/// Ping handler - demonstrates POST with default method (no method attribute)
#[route("/ping", version = "v1", tag = "Health", summary = "Ping endpoint")]
async fn handle_ping(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    // Access request body fields
    let msg = req.body.message.as_deref().unwrap_or("no message");
    let value = req.body.value;

    println!("[{}] Received ping: {} at {}", req.request_id(), msg, value);

    // Build response with plain struct
    Ok(Response::ok(TestResponse {
        message: Some(format!("Pong! Got: {}", msg)),
        value: value + 1,
        success: true,
    }))
}

/// Health check handler - demonstrates GET method
#[route(
    "/health",
    method = "GET",
    version = "v1",
    tag = "Health",
    summary = "Health check"
)]
async fn handle_health(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    println!("[{}] Health check at: {}", req.request_id(), req.body.value);

    Ok(Response::ok(TestResponse {
        message: Some("OK".to_string()),
        value: req.body.value,
        success: true,
    }))
}

/// Create handler - demonstrates explicit POST method
#[route(
    "/create",
    method = "POST",
    version = "v1",
    tag = "Resources",
    summary = "Create resource"
)]
async fn handle_create(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    println!(
        "[{}] Creating resource: {:?}",
        req.request_id(),
        req.body.message
    );

    Ok(Response::created(TestResponse {
        message: Some("Created".to_string()),
        value: req.body.value,
        success: true,
    }))
}

/// Update handler - demonstrates PUT method
#[route(
    "/update",
    method = "PUT",
    version = "v1",
    tag = "Resources",
    summary = "Update resource"
)]
async fn handle_update(
    req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    println!(
        "[{}] Updating resource: {:?}",
        req.request_id(),
        req.body.message
    );

    Ok(Response::ok(TestResponse {
        message: Some("Updated".to_string()),
        value: req.body.value,
        success: true,
    }))
}

/// Error handler - demonstrates error responses
#[route(
    "/error",
    version = "v1",
    tag = "Test",
    summary = "Error test endpoint"
)]
async fn handle_error(
    _req: Request<TestRequest>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    // Demonstrate different error types
    Err(FlatbedRouteError::bad_request("This is a test error")
        .code("TEST_ERROR")
        .header("x-error-type", "test"))
}

fn main() {
    println!("Flatbed Route Decorator System Example");
    println!("=====================================\n");

    // Validate all routes are unique
    match validate_routes() {
        Ok(()) => println!("✓ All routes validated successfully\n"),
        Err(conflict) => {
            eprintln!("✗ Route conflict detected: {}", conflict);
            std::process::exit(1);
        }
    }

    // Get the route map for inspection
    let routes = get_routes();

    println!("Registered routes:");
    println!("------------------");
    for ((path, _method), route_info) in routes.iter() {
        println!(
            "  {} {} -> ({}, {})",
            route_info.method, path, route_info.request_type, route_info.response_type
        );
        if let Some(version) = route_info.openapi.version {
            println!("       version: {}", version);
        }
        if let Some(tag) = route_info.openapi.tag {
            println!("       tag: {}", tag);
        }
        if let Some(summary) = route_info.openapi.summary {
            println!("       summary: {}", summary);
        }
    }

    println!("\nTotal routes: {}\n", routes.len());

    // Note: This example just demonstrates route registration.
    // For a full server example, use #[flatbed::main]:
    //
    // #[flatbed::main(bind = "0.0.0.0:8080")]
    // async fn main() {
    //     println!("Server started");
    // }
    println!("Routes registered successfully!");
    println!("\nTo run a server, use #[flatbed::main] macro:");
    println!("  #[flatbed::main(bind = \"0.0.0.0:8080\")]");
    println!("  async fn main() {{");
    println!("      println!(\"Server started\");");
    println!("  }}");
}
