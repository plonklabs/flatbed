//! End-to-end check of a registered `LocalQueueWorker` under `Flatbed::run`
//! with a `FlatbedContext`: messages sent while the process does not lead
//! wait, are handled in order with duplicates coalesced once it leads, and the
//! worker reports `running`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flatbed::local_queue::{HasLocalQueues, KeyStatus, LocalQueueResult, LocalQueueWorker};
use flatbed::{Flatbed, FlatbedConfig, FlatbedContext};
use tokio::sync::watch;

#[derive(Clone, Default)]
struct AppData {
    handled: Arc<Mutex<Vec<String>>>,
}

type AppContext = FlatbedContext<AppData>;

/// A context whose NATS and kube clients never connect: nothing in this test
/// reaches either.
async fn app_context() -> AppContext {
    let nats = flatbed::async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("127.0.0.1:1")
        .await
        .unwrap();
    let kube_config = flatbed::kube::Config::new("http://127.0.0.1:1".parse().unwrap());
    let (is_leader_tx, is_leader_rx) = watch::channel(false);
    FlatbedContext::builder(AppData::default())
        .jetstream(flatbed::async_nats::jetstream::new(nats.clone()))
        .nats_client(nats)
        .kube_client(flatbed::kube::Client::try_from(kube_config).unwrap())
        .leader_election(Arc::new(is_leader_tx), is_leader_rx, true)
        .build()
}

#[derive(Clone)]
struct SetupNamespace {
    namespace: &'static str,
    server: &'static str,
    attempt: u32,
}

#[derive(Default)]
struct NamespaceSetup;

impl LocalQueueWorker for NamespaceSetup {
    type Message = SetupNamespace;
    type Key = (&'static str, &'static str);
    type Context = AppContext;
    type Error = String;

    const NAME: &'static str = "namespace-setup";

    fn key(msg: &SetupNamespace) -> Self::Key {
        (msg.namespace, msg.server)
    }

    fn handle(
        &self,
        ctx: Arc<AppContext>,
        msg: SetupNamespace,
    ) -> flatbed::BoxFuture<Result<LocalQueueResult, String>> {
        Box::pin(async move {
            ctx.handled
                .lock()
                .unwrap()
                .push(format!("{}@{}#{}", msg.namespace, msg.server, msg.attempt));
            Ok(LocalQueueResult::Done)
        })
    }
}

flatbed::register_local_queue_worker!(NamespaceSetup, AppContext);

fn running(metrics: &str) -> bool {
    metrics.lines().any(|line| {
        line.starts_with("flatbed_worker_state{")
            && line.contains(r#"worker="namespace-setup""#)
            && line.contains(r#"state="running""#)
            && line.ends_with(" 1")
    })
}

#[tokio::test]
async fn registered_local_queue_worker_handles_queued_messages_once_leading() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let telemetry_config = flatbed::TelemetryConfig::new(
        Some("local-queue-worker-test".to_string()),
        Some("127.0.0.1".to_string()),
        Some(port),
    )
    .unwrap();
    let telemetry: Arc<dyn flatbed::TelemetryService> = <flatbed::telemetry::prometheus::PrometheusTelemetryService as flatbed::TelemetryService>::new(
        telemetry_config,
    );
    let config = FlatbedConfig::new("Local Queue Worker Test")
        .host("127.0.0.1")
        .port(port)
        .with_telemetry(telemetry);

    let ctx = app_context().await;
    let boot_ctx = ctx.clone();
    let server =
        tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(boot_ctx) }).await });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut metrics = String::new();
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let Ok(response) = client.get(format!("{base}/metrics")).send().await else {
            continue;
        };
        metrics = response.text().await.unwrap_or_default();
        if running(&metrics) {
            break;
        }
    }
    assert!(
        running(&metrics),
        "worker not running; metrics were:\n{metrics}"
    );

    let send = |namespace, server, attempt| {
        ctx.local_queue::<NamespaceSetup>()
            .send(SetupNamespace {
                namespace,
                server,
                attempt,
            })
            .unwrap();
    };
    send("team-a", "s1", 1);
    send("team-b", "s1", 1);
    send("team-a", "s1", 2);
    assert_eq!(
        ctx.local_queue::<NamespaceSetup>()
            .status(&("team-b", "s1")),
        KeyStatus::Queued(2)
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        ctx.handled.lock().unwrap().is_empty(),
        "nothing is handled before the process leads"
    );

    ctx.is_leader_tx.send_replace(true);
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if ctx.handled.lock().unwrap().len() >= 2 {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(*ctx.handled.lock().unwrap(), ["team-a@s1#1", "team-b@s1#1"],);
    assert_eq!(
        ctx.local_queue::<NamespaceSetup>()
            .status(&("team-a", "s1")),
        KeyStatus::Absent
    );

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    assert_eq!(health.text().await.unwrap(), "OK");

    server.abort();
}
