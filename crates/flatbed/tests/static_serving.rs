//! Integration test for `static_route!` serving over a real HTTP connection.
//!
//! This binary registers only static mounts (no routes or workers), so the
//! server boots cleanly and the static path is exercised end to end.

use flatbed::{static_route, Flatbed, FlatbedConfig};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

mod common;

// `dir` resolves relative to the crate root (cargo test's working directory).
static_route!(mount = "/static", dir = "tests/static_fixture");
// The same files, compiled into this test binary.
static_route!(
    mount = "/embedded",
    embed = "tests/static_fixture",
    fallback = "index.html",
    no_fallback = ["/embedded/api/"]
);

/// Start a server on a free port and wait until static serving answers,
/// returning the port and the server task.
async fn boot(client: &reqwest::Client) -> (u16, JoinHandle<Result<(), flatbed::Error>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = FlatbedConfig::new("static-test")
        .host("127.0.0.1")
        .port(port);
    let server = tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    // Static serving returns 503 until the boot function completes; wait it out.
    let url = format!("http://127.0.0.1:{port}/static/app.js");
    for _ in 0..100 {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().as_u16() != 503 {
                return (port, server);
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

/// A HEAD request to a static file returns the same `Content-Length` header
/// as GET but no body, over both HTTP/1.1 and HTTP/2.
#[tokio::test]
async fn head_static_file_strips_body_keeps_length() {
    let client = reqwest::Client::new();
    let (port, server) = boot(&client).await;
    let url = format!("http://127.0.0.1:{port}/static/app.js");

    // HTTP/1.1, via reqwest: same Content-Length as GET, no client-visible body.
    let get = client.get(&url).send().await.unwrap();
    assert_eq!(get.status().as_u16(), 200);
    assert_eq!(
        get.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    let get_content_length = get
        .headers()
        .get("content-length")
        .expect("GET content-length present")
        .to_str()
        .unwrap()
        .to_string();

    let head = client.head(&url).send().await.unwrap();
    assert_eq!(head.status().as_u16(), 200);
    assert_eq!(
        head.headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        get_content_length,
        "HEAD Content-Length must equal GET's over HTTP/1.1"
    );

    // HTTP/2 cleartext: no protocol error, same content-length, zero actual
    // body bytes received.
    let (status, content_length, received) = common::h2c_head_request(port, "/static/app.js").await;
    assert_eq!(status, http::StatusCode::OK, "HEAD should be 200 over h2c");
    assert_eq!(
        content_length.as_deref(),
        Some(get_content_length.as_str()),
        "HEAD content-length must equal GET's over h2c"
    );
    assert_eq!(received, 0, "HEAD must not put a body on the wire over h2c");

    server.abort();
}

/// An embedded mount serves the compiled-in bytes with the same content type
/// as the filesystem mount, and its fallback answers a client-side route.
#[tokio::test]
async fn embedded_mount_serves_from_the_binary() {
    let client = reqwest::Client::new();
    let (port, server) = boot(&client).await;

    let asset = client
        .get(format!("http://127.0.0.1:{port}/embedded/app.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status().as_u16(), 200);
    assert_eq!(
        asset.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        asset.headers().get("cache-control").unwrap(),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        asset.bytes().await.unwrap().as_ref(),
        std::fs::read("tests/static_fixture/app.js").unwrap()
    );

    let shell = client
        .get(format!(
            "http://127.0.0.1:{port}/embedded/some/client/route"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(shell.status().as_u16(), 200);
    assert_eq!(
        shell.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    assert!(shell.text().await.unwrap().contains("embedded shell"));

    let missing = client
        .get(format!("http://127.0.0.1:{port}/embedded/missing.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);

    // A miss under a `no_fallback` prefix is a 404, not the shell.
    let api_miss = client
        .get(format!("http://127.0.0.1:{port}/embedded/api/nothing"))
        .send()
        .await
        .unwrap();
    assert_eq!(api_miss.status().as_u16(), 404);

    server.abort();
}
