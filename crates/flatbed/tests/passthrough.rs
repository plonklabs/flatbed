//! A `Request<Passthrough>` route receives the request body as sent, under
//! either wire format, without a copy after hyper reads it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use flatbed::bytes::Bytes;
use flatbed::{route, Flatbed, FlatbedConfig, FlatbedRouteError, Passthrough, Request, Response};

static RECEIVED_AT: AtomicUsize = AtomicUsize::new(0);

#[route("/echo", method = "POST")]
async fn echo(req: Request<Passthrough>) -> Result<Response<()>, FlatbedRouteError> {
    RECEIVED_AT.store(req.body.0.as_ptr() as usize, Ordering::SeqCst);
    Ok(Response::raw(
        req.body.0.to_vec(),
        "application/octet-stream",
    ))
}

#[tokio::test]
async fn the_handler_holds_the_bytes_it_was_given() {
    for content_type in ["application/json", "application/x-flatbuffers"] {
        let body = Bytes::from_static(b"{\"model\": \"gpt\",   \"stream\": true}");
        let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/echo".to_string());
        __hyper_handler_echo(parts, body.clone(), content_type, Arc::new(()))
            .await
            .expect("echo succeeds");
        assert_eq!(
            RECEIVED_AT.load(Ordering::SeqCst),
            body.as_ptr() as usize,
            "{content_type}: the body was copied on its way to the handler"
        );
    }
}

#[tokio::test]
async fn a_passthrough_route_echoes_the_body_as_sent() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = FlatbedConfig::new("passthrough")
        .host("127.0.0.1")
        .port(port);
    tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/echo");
    let send = |content_type: &'static str, body: &'static [u8]| {
        client
            .post(&url)
            .header("content-type", content_type)
            .body(body)
            .send()
    };
    for _ in 0..100 {
        if send("application/json", b"{}")
            .await
            .is_ok_and(|r| r.status().as_u16() == 200)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let json: &[u8] = b"{ \"model\":\"gpt-5\" ,\n \"input\": [1, 2.50, 3e2] }";
    let echoed = send("application/json", json).await.unwrap();
    assert_eq!(echoed.status(), 200);
    assert_eq!(echoed.bytes().await.unwrap(), json);

    let flatbuffer: &[u8] = &[0x0c, 0x00, 0x00, 0x00, 0xff, 0x00, 0x7f, 0x80];
    let echoed = send("application/x-flatbuffers", flatbuffer).await.unwrap();
    assert_eq!(echoed.status(), 200);
    assert_eq!(echoed.bytes().await.unwrap(), flatbuffer);

    let refused = send("text/plain", b"hello").await.unwrap();
    assert_eq!(refused.status(), 415);
}
