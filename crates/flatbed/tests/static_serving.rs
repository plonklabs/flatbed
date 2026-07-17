//! Integration test for `static_route!` serving over a real HTTP connection.
//!
//! This binary registers only a static mount (no routes or workers), so the
//! server boots cleanly and the static path is exercised end to end.

use flatbed::{static_route, Flatbed, FlatbedConfig};
use tokio::time::{sleep, Duration};

// `dir` resolves relative to the crate root (cargo test's working directory).
static_route!(mount = "/static", dir = "tests/static_fixture");

/// A HEAD request to a static file returns the same `Content-Length` as GET but
/// no body: hyper strips the body at the connection layer while the full-read
/// path still reports the correct length.
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

    let content_length = |r: &reqwest::Response| -> String {
        r.headers()
            .get("content-length")
            .expect("content-length present")
            .to_str()
            .unwrap()
            .to_string()
    };

    // GET: full body, content-type from extension, Content-Length == body size.
    let get = client.get(&url).send().await.unwrap();
    assert_eq!(get.status().as_u16(), 200);
    assert_eq!(
        get.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    let get_len = content_length(&get);
    let get_body = get.bytes().await.unwrap();
    assert!(!get_body.is_empty());
    assert_eq!(get_len, get_body.len().to_string());

    // HEAD: same Content-Length header, no body on the wire.
    let head = client.head(&url).send().await.unwrap();
    assert_eq!(head.status().as_u16(), 200);
    assert_eq!(
        head.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        content_length(&head),
        get_len,
        "HEAD Content-Length must equal GET's"
    );
    assert!(
        head.bytes().await.unwrap().is_empty(),
        "HEAD must not send a body"
    );

    server.abort();
}
