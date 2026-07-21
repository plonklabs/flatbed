//! `/schema.bfbs` returns 404 when no schema is compiled into the service.
//! This binary deliberately omits `mod generated`, so the `FlatbedSchema`
//! registry is empty and `get_schema_bfbs()` is `None`.

use flatbed::{Flatbed, FlatbedConfig};

#[tokio::test]
async fn schema_endpoint_is_404_without_a_compiled_schema() {
    assert!(
        flatbed::get_schema_bfbs().is_none(),
        "no generated schema is linked into this test binary"
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = FlatbedConfig::new("No Schema").host("127.0.0.1").port(port);
    tokio::spawn(async move { Flatbed::run(config, |_config| async move { Ok(()) }).await });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let resp = client
        .get(format!("{base}/schema.bfbs"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        404,
        "/schema.bfbs is 404 when no schema is compiled in"
    );
}
