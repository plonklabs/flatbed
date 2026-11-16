//! A route returning `Response::stream` sends each chunk as the stream yields
//! it, records how the stream ended exactly once, and a HEAD on such a route
//! answers without running the stream.
//!
//! The log capture and the error reporter are process-wide, so each test
//! picks out its own records by `x-request-id`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

mod common;

use std::convert::Infallible;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use flatbed::bytes::Bytes;
use flatbed::{
    route, ErrorReporter, Flatbed, FlatbedConfig, FlatbedRouteError, PanicReport, Request,
    Response, RouteFailure, WorkerFailure,
};
use futures::{Stream, StreamExt};
use generated::test::Defaulted;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNKS: usize = 5;
const CHUNK_GAP: Duration = Duration::from_millis(200);

static HEAD_STREAM_POLLED: AtomicBool = AtomicBool::new(false);
static HOLD_DROPPED: AtomicBool = AtomicBool::new(false);

/// Every log line written in this binary.
static LOGS: LazyLock<Arc<Mutex<Vec<u8>>>> = LazyLock::new(|| {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&logs);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || LogWriter(Arc::clone(&sink)))
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    logs
});

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The `stream ended` lines logged for `request_id`.
fn stream_ended(request_id: &str) -> Vec<String> {
    let logs = LOGS.lock().unwrap();
    String::from_utf8_lossy(&logs)
        .lines()
        .filter(|line| line.contains("stream ended") && line.contains(request_id))
        .map(str::to_string)
        .collect()
}

/// Wait for `request_id`'s `stream ended` line, then a moment more so a
/// second one would have landed too.
async fn settled_stream_ended(request_id: &str) -> Vec<String> {
    for _ in 0..50 {
        if !stream_ended(request_id).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    stream_ended(request_id)
}

#[derive(Debug, Clone, PartialEq)]
struct Reported {
    request_id: String,
    route: String,
    status: u16,
    code: Option<String>,
    message: String,
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Reported>>);

impl ErrorReporter for Recorder {
    fn route_error(&self, failure: &RouteFailure<'_>) {
        self.0.lock().unwrap().push(Reported {
            request_id: failure.request.request_id.to_string(),
            route: failure.request.route.to_string(),
            status: failure.status,
            code: failure.code.map(str::to_string),
            message: failure.message.to_string(),
        });
    }

    fn panic(&self, _panic: &PanicReport<'_>) {}

    fn worker_failure(&self, _failure: &WorkerFailure<'_>) {}
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);

fn reported(request_id: &str) -> Vec<Reported> {
    let records = RECORDER.0.lock().unwrap();
    records
        .iter()
        .filter(|record| record.request_id == request_id)
        .cloned()
        .collect()
}

/// `CHUNKS` chunks, the first at once and each later one `CHUNK_GAP` after
/// the one before.
fn spaced_chunks() -> impl Stream<Item = Result<Bytes, Infallible>> + Send {
    futures::stream::unfold(0, |i| async move {
        if i == CHUNKS {
            return None;
        }
        if i > 0 {
            tokio::time::sleep(CHUNK_GAP).await;
        }
        Some((Ok(Bytes::from(format!("chunk {i}\n"))), i + 1))
    })
}

#[route("/events", method = "GET")]
async fn events(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    Ok(Response::stream("text/event-stream", spaced_chunks()))
}

#[route("/events", method = "HEAD")]
async fn events_head(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let stream = futures::stream::poll_fn(|_| {
        HEAD_STREAM_POLLED.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, Infallible>(Bytes::from_static(b"never"))))
    });
    Ok(Response::stream("text/event-stream", stream))
}

/// Sets `HOLD_DROPPED` when the stream that owns it is dropped.
struct DropGuard;

impl Drop for DropGuard {
    fn drop(&mut self) {
        HOLD_DROPPED.store(true, Ordering::SeqCst);
    }
}

/// One chunk, then nothing until the client goes away.
#[route("/hold", method = "GET")]
async fn hold(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let guard = DropGuard;
    let stream = futures::stream::once(async { Ok(Bytes::from_static(b"first\n")) })
        .chain(futures::stream::pending::<Result<Bytes, Infallible>>())
        .map(move |chunk| {
            let _owned = &guard;
            chunk
        });
    Ok(Response::stream("text/event-stream", stream))
}

/// One chunk, then an error once the chunk has had time to reach the client.
#[route("/fails", method = "GET")]
async fn fails(_req: Request<Defaulted>) -> Result<Response<()>, FlatbedRouteError> {
    let stream = futures::stream::once(async { Ok(Bytes::from_static(b"first\n")) }).chain(
        futures::stream::once(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(std::io::Error::other("upstream reset"))
        }),
    );
    Ok(Response::stream("text/event-stream", stream))
}

/// Start a server on a free port and return it once its routes answer.
async fn serve() -> u16 {
    LazyLock::force(&LOGS);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let reporter: Arc<dyn ErrorReporter> = RECORDER.clone();
    let config = FlatbedConfig::new("streaming")
        .host("127.0.0.1")
        .port(port)
        .with_error_reporter(reporter);
    tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/events");
    for _ in 0..100 {
        if let Ok(r) = client.head(&url).send().await {
            if r.status().as_u16() == 200 {
                return port;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

#[tokio::test]
async fn a_streamed_reply_sends_its_first_chunk_before_its_last() {
    let port = serve().await;

    let started = Instant::now();
    let mut response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/events"))
        .header("x-request-id", "ttfb-1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert!(response.headers().get("x-request-id").is_some());

    let first = response.chunk().await.unwrap().expect("a first chunk");
    let first_after = started.elapsed();
    assert_eq!(first, "chunk 0\n");
    assert!(
        first_after < CHUNK_GAP,
        "the first chunk took {first_after:?}, so the body was buffered"
    );

    let mut body = first.to_vec();
    while let Some(chunk) = response.chunk().await.unwrap() {
        body.extend_from_slice(&chunk);
    }
    let expected: String = (0..CHUNKS).map(|i| format!("chunk {i}\n")).collect();
    assert_eq!(String::from_utf8(body).unwrap(), expected);

    let ended = settled_stream_ended("ttfb-1");
    let ended = ended.await;
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(ended[0].contains("completed"), "{}", ended[0]);
    assert!(
        ended[0].contains(&format!("bytes={}", expected.len())),
        "{}",
        ended[0]
    );
}

/// Send a bare HTTP/1.1 GET and return the open connection.
async fn raw_get(port: u16, path: &str, request_id: &str) -> tokio::net::TcpStream {
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request =
        format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nx-request-id: {request_id}\r\n\r\n");
    tcp.write_all(request.as_bytes()).await.unwrap();
    tcp
}

#[tokio::test]
async fn a_client_disconnect_drops_the_stream_and_ends_it_once_as_disconnected() {
    let port = serve().await;

    let mut tcp = raw_get(port, "/hold", "hold-1").await;
    let mut received = Vec::new();
    while !String::from_utf8_lossy(&received).contains("first") {
        let mut buf = [0u8; 1024];
        let n = tcp.read(&mut buf).await.unwrap();
        assert!(n > 0, "the connection closed before the first chunk");
        received.extend_from_slice(&buf[..n]);
    }
    drop(tcp);

    let dropped_by = Instant::now() + Duration::from_secs(1);
    while !HOLD_DROPPED.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < dropped_by,
            "the stream outlived its client by a second"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let ended = settled_stream_ended("hold-1").await;
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(ended[0].contains("disconnected"), "{}", ended[0]);
    assert!(ended[0].contains("bytes=6"), "{}", ended[0]);
}

#[tokio::test]
async fn a_stream_error_is_reported_once_and_aborts_the_http1_transfer() {
    let port = serve().await;

    let mut tcp = raw_get(port, "/fails", "fails-1").await;
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), tcp.read_to_end(&mut received))
        .await
        .expect("the server closed the connection")
        .unwrap();
    let received = String::from_utf8_lossy(&received);
    assert!(received.starts_with("HTTP/1.1 200"), "{received}");
    assert!(received.contains("first"), "{received}");
    assert!(
        !received.ends_with("0\r\n\r\n"),
        "an aborted transfer must not send the final chunk: {received:?}"
    );

    let ended = settled_stream_ended("fails-1").await;
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert!(ended[0].contains("failed"), "{}", ended[0]);
    assert_eq!(
        reported("fails-1"),
        vec![Reported {
            request_id: "fails-1".to_string(),
            route: "/fails".to_string(),
            status: 200,
            code: Some("STREAM_ERROR".to_string()),
            message: "upstream reset".to_string(),
        }]
    );
}

#[tokio::test]
async fn a_stream_error_resets_the_http2_stream() {
    let port = serve().await;

    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut client, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(connection);
    let request = http::Request::get("/fails")
        .header("x-request-id", "fails-h2")
        .body(())
        .unwrap();
    let (response, _) = client.send_request(request, true).unwrap();
    let response = response.await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);

    let mut body = response.into_body();
    let mut reset = false;
    while let Some(chunk) = body.data().await {
        if chunk.is_err() {
            reset = true;
            break;
        }
    }
    assert!(
        reset,
        "the HTTP/2 stream ended cleanly after a stream error"
    );
    assert_eq!(reported("fails-h2").len(), 1);
}

#[tokio::test]
async fn a_head_on_a_streaming_route_answers_without_running_the_stream() {
    let port = serve().await;

    let head = reqwest::Client::new()
        .head(format!("http://127.0.0.1:{port}/events"))
        .header("x-request-id", "head-1")
        .send()
        .await
        .unwrap();
    assert_eq!(head.status().as_u16(), 200);
    assert_eq!(head.headers().get("content-length"), None);

    let (status, content_length, received) = common::h2c_head_request(port, "/events").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(content_length, None);
    assert_eq!(received, 0);

    assert!(
        !HEAD_STREAM_POLLED.load(Ordering::SeqCst),
        "a HEAD polled the stream"
    );
    assert_eq!(settled_stream_ended("head-1").await, Vec::<String>::new());
}
