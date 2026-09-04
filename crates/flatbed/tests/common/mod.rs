//! Shared test helpers for `flatbed`'s integration test binaries.

/// Send a HEAD request over a fresh HTTP/2 cleartext (h2c, prior-knowledge)
/// connection and return the response status, its `content-length` header,
/// and the number of body bytes actually received on the wire.
///
/// A client library discards a HEAD response's body per HTTP semantics, so
/// asserting against a client-parsed body can't tell a fixed server from a
/// broken one — an HTTP/1.1 client won't even surface the framing error a
/// broken server produces. `hyper`'s HTTP/1.1 encoder special-cases HEAD
/// (suppressing the body while still deriving `content-length` from it), but
/// its HTTP/2 encoder does not: a response built with a real body is sent
/// with one, which a compliant HTTP/2 client (this one included) rejects as
/// `PROTOCOL_ERROR` because content-length promised zero bytes past headers.
/// Speaking raw h2c is the only way to observe this at the transport the
/// framework actually serves.
pub async fn h2c_head_request(port: u16, path: &str) -> (http::StatusCode, Option<String>, usize) {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut client, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(connection);

    let request = http::Request::builder()
        .method("HEAD")
        .uri(path)
        .body(())
        .unwrap();
    let (response_fut, _send_stream) = client.send_request(request, true).unwrap();
    let response = response_fut.await.unwrap();

    let status = response.status();
    let content_length = response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .map(|v| v.to_str().unwrap().to_string());

    let mut body = response.into_body();
    let mut received = 0usize;
    while let Some(chunk) = body.data().await {
        received += chunk.unwrap().len();
    }

    (status, content_length, received)
}
