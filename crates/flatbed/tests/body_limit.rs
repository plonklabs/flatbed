//! A request body past the route's limit is answered `413 PAYLOAD_TOO_LARGE`
//! in the request's wire format: before the body is read when
//! `content-length` declares it, and as soon as a chunked body grows past it.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::time::Duration;

use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};
use generated::test::Defaulted;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SERVICE_LIMIT: usize = 1024;
const ROUTE_LIMIT: usize = 5 * 1024 * 1024;

#[route("/small", method = "POST")]
async fn small(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    Ok(Response::raw(b"ok".to_vec(), "text/plain"))
}

#[route("/upload", method = "POST", max_body = 5 * 1024 * 1024)]
async fn upload(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    Ok(Response::raw(b"ok".to_vec(), "text/plain"))
}

/// A JSON `Defaulted` padded with whitespace to exactly `len` bytes.
fn json_body(len: usize) -> Vec<u8> {
    let mut body = br#"{"count":1}"#.to_vec();
    body.resize(len, b' ');
    body
}

async fn serve() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = FlatbedConfig::new("body-limit")
        .host("127.0.0.1")
        .port(port)
        .max_request_body(SERVICE_LIMIT);
    tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let client = reqwest::Client::new();
    for _ in 0..100 {
        let answer = client
            .post(format!("http://127.0.0.1:{port}/small"))
            .header("content-type", "application/json")
            .body(json_body(16))
            .send()
            .await;
        if answer.is_ok_and(|r| r.status().as_u16() == 200) {
            return port;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

async fn post(port: u16, path: &str, body: Vec<u8>) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}{path}"))
        .header("content-type", "application/json")
        .header("x-request-id", "limit-1")
        .body(body)
        .send()
        .await
        .unwrap()
}

/// Read from `tcp` until the response head is in, or the connection closes.
async fn read_head(tcp: &mut tokio::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut buf = [0u8; 4096];
    while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut buf))
            .await
            .expect("the server answered")
            .unwrap();
        if n == 0 {
            break;
        }
        received.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&received).into_owned()
}

#[tokio::test]
async fn a_body_at_the_limit_passes_and_one_byte_over_is_413() {
    let port = serve().await;

    assert_eq!(
        post(port, "/small", json_body(SERVICE_LIMIT))
            .await
            .status(),
        200
    );

    let over = post(port, "/small", json_body(SERVICE_LIMIT + 1)).await;
    assert_eq!(over.status(), 413);
    assert_eq!(over.headers().get("x-request-id").unwrap(), "limit-1");
    let body: serde_json::Value = over.json().await.unwrap();
    assert_eq!(body["code"], "PAYLOAD_TOO_LARGE");
}

#[tokio::test]
async fn a_route_max_body_overrides_the_service_limit() {
    let port = serve().await;

    assert_eq!(
        post(port, "/upload", json_body(2 * 1024 * 1024))
            .await
            .status(),
        200
    );
    assert_eq!(
        post(port, "/upload", json_body(ROUTE_LIMIT)).await.status(),
        200
    );
}

#[tokio::test]
async fn a_413_speaks_the_request_wire_format() {
    let port = serve().await;

    let over = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/small"))
        .header("content-type", "application/x-flatbuffers")
        .header("x-request-id", "limit-fb")
        .body(vec![0u8; SERVICE_LIMIT + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 413);
    assert_eq!(
        over.headers().get("content-type").unwrap(),
        "application/x-flatbuffers"
    );
    assert_eq!(over.headers().get("x-request-id").unwrap(), "limit-fb");
    assert_eq!(
        over.headers().get("x-error-code").unwrap(),
        "PAYLOAD_TOO_LARGE"
    );
}

#[tokio::test]
async fn a_declared_upload_over_the_limit_is_413_before_a_body_byte_with_expect_continue() {
    let port = serve().await;

    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let head = format!(
        "POST /upload HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nexpect: 100-continue\r\nx-request-id: limit-expect\r\n\r\n",
        7 * 1024 * 1024
    );
    tcp.write_all(head.as_bytes()).await.unwrap();

    let answer = read_head(&mut tcp).await;
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
    assert!(answer.contains("x-request-id: limit-expect"), "{answer}");
}

#[tokio::test]
async fn a_declared_upload_over_the_limit_without_expect_never_hangs() {
    let port = serve().await;

    let sent = tokio::time::timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/upload"))
            .header("content-type", "application/json")
            .body(json_body(7 * 1024 * 1024))
            .send(),
    )
    .await
    .expect("the upload hung");
    if let Ok(answer) = sent {
        assert_eq!(answer.status(), 413);
    }
}

#[tokio::test]
async fn a_chunked_body_growing_past_the_limit_is_413() {
    let port = serve().await;

    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let head = "POST /small HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
                transfer-encoding: chunked\r\nx-request-id: limit-chunked\r\n\r\n";
    tcp.write_all(head.as_bytes()).await.unwrap();
    let chunk = json_body(SERVICE_LIMIT / 2);
    for _ in 0..3 {
        let framed = [
            format!("{:x}\r\n", chunk.len()).into_bytes(),
            chunk.clone(),
            b"\r\n".to_vec(),
        ]
        .concat();
        if tcp.write_all(&framed).await.is_err() {
            break;
        }
    }

    let answer = read_head(&mut tcp).await;
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
    assert!(answer.contains("x-request-id: limit-chunked"), "{answer}");
}
