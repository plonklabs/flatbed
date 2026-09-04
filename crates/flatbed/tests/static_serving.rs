//! Integration test for `static_route!` serving over a real HTTP connection.
//!
//! This binary registers only a static mount (no routes or workers), so the
//! server boots cleanly and the static path is exercised end to end.

use flatbed::{static_route, Flatbed, FlatbedConfig};
use tokio::time::{sleep, Duration};

mod common;

// `dir` resolves relative to the crate root (cargo test's working directory).
static_route!(mount = "/static", dir = "tests/static_fixture");

/// A HEAD request to a static file returns the same `Content-Length` header
/// as GET but no body, over both HTTP/1.1 and HTTP/2.
#[tokio::test]
async fn head_static_file_strips_body_keeps_length() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = FlatbedConfig::new("static-test")
        .host("127.0.0.1")
        .port(port);
    let server = tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let url = format!("http://127.0.0.1:{}/static/app.js", port);
    let client = reqwest::Client::new();

    // Static serving returns 503 until the boot function completes; wait it out.
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().as_u16() != 503 {
                ready = true;
                break;
            }
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "server did not become ready");

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
