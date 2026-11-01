//! End-to-end check of a registered `MessageWorker` under `Flatbed::run`:
//! messages sent while the process does not lead wait, are handled in order
//! with duplicates coalesced once it leads, and the worker reports `running`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flatbed::k8s::HasLeaderElection;
use flatbed::message::{MessageResult, MessageSender, MessageWorker};
use flatbed::{Flatbed, FlatbedConfig};
use tokio::sync::watch;

#[derive(Clone)]
struct AppContext {
    setups: MessageSender<NamespaceSetup>,
    handled: Arc<Mutex<Vec<String>>>,
    leader: Arc<watch::Sender<bool>>,
}

impl HasLeaderElection for AppContext {
    fn is_leader_rx(&self) -> watch::Receiver<bool> {
        self.leader.subscribe()
    }
}

#[derive(Clone)]
struct SetupNamespace {
    namespace: &'static str,
    server: &'static str,
    attempt: u32,
}

#[derive(Default)]
struct NamespaceSetup;

impl MessageWorker for NamespaceSetup {
    type Message = SetupNamespace;
    type Key = (&'static str, &'static str);
    type Context = AppContext;
    type Error = String;

    const NAME: &'static str = "namespace-setup";

    fn key(msg: &SetupNamespace) -> Self::Key {
        (msg.namespace, msg.server)
    }

    fn sender(ctx: &AppContext) -> &MessageSender<Self> {
        &ctx.setups
    }

    fn handle(
        &self,
        ctx: Arc<AppContext>,
        msg: SetupNamespace,
    ) -> flatbed::BoxFuture<Result<MessageResult, String>> {
        Box::pin(async move {
            ctx.handled
                .lock()
                .unwrap()
                .push(format!("{}@{}#{}", msg.namespace, msg.server, msg.attempt));
            Ok(MessageResult::Done)
        })
    }
}

flatbed::register_message_worker!(NamespaceSetup, AppContext);

fn running(metrics: &str) -> bool {
    metrics.lines().any(|line| {
        line.starts_with("flatbed_worker_state{")
            && line.contains(r#"worker="namespace-setup""#)
            && line.contains(r#"state="running""#)
            && line.ends_with(" 1")
    })
}

#[tokio::test]
async fn registered_message_worker_handles_queued_messages_once_leading() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let telemetry_config = flatbed::TelemetryConfig::new(
        Some("message-worker-test".to_string()),
        Some("127.0.0.1".to_string()),
        Some(port),
    )
    .unwrap();
    let telemetry: Arc<dyn flatbed::TelemetryService> = <flatbed::telemetry::prometheus::PrometheusTelemetryService as flatbed::TelemetryService>::new(
        telemetry_config,
    );
    let config = FlatbedConfig::new("Message Worker Test")
        .host("127.0.0.1")
        .port(port)
        .with_telemetry(telemetry);

    let ctx = AppContext {
        setups: MessageSender::new(),
        handled: Arc::default(),
        leader: Arc::new(watch::channel(false).0),
    };
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
        ctx.setups
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
    assert_eq!(ctx.setups.position(&("team-b", "s1")), Some(2));

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        ctx.handled.lock().unwrap().is_empty(),
        "nothing is handled before the process leads"
    );

    ctx.leader.send_replace(true);
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if ctx.handled.lock().unwrap().len() >= 2 {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(*ctx.handled.lock().unwrap(), ["team-a@s1#1", "team-b@s1#1"],);
    assert_eq!(ctx.setups.position(&("team-a", "s1")), None);

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    assert_eq!(health.text().await.unwrap(), "OK");

    server.abort();
}
