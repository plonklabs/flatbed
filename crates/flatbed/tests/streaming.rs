//! A route returning `Response::stream` sends each chunk as the stream yields
//! it, and a HEAD on such a route answers without running the stream.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

mod common;

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flatbed::bytes::Bytes;
use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Request, Response};
use futures::Stream;
use generated::test::Defaulted;

const CHUNKS: usize = 5;
const CHUNK_GAP: Duration = Duration::from_millis(200);

static HEAD_STREAM_POLLED: AtomicBool = AtomicBool::new(false);

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

/// Start a server on a free port and return it once its routes answer.
async fn serve() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = FlatbedConfig::new("streaming").host("127.0.0.1").port(port);
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
    let mut response = reqwest::get(format!("http://127.0.0.1:{port}/events"))
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
}

#[tokio::test]
async fn a_head_on_a_streaming_route_answers_without_running_the_stream() {
    let port = serve().await;

    let head = reqwest::Client::new()
        .head(format!("http://127.0.0.1:{port}/events"))
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
}
