//! Broker-backed integration tests for the managed connector: connect with
//! retry against a broker that is not reachable yet, and `/readyz` tracking
//! the connection across a severed and re-established link.
//!
//! The link is cut at a TCP proxy the test owns rather than by stopping the
//! broker, so the tests need no control over the container they are pointed
//! at and one broker can serve every seat running them.
//!
//! The tests are marked `#[ignore]` so the plain workspace test run stays
//! broker-free. `scripts/nats-broker.sh` starts a broker scoped to the
//! current worktree and prints its address:
//!
//! ```text
//! scripts/nats-broker.sh up
//! NATS_URL=$(scripts/nats-broker.sh url) \
//!   cargo test -p flatbed --features nats,openapi,telemetry \
//!     --test nats_connect_broker -- --ignored
//! ```
//!
//! `NATS_URL` defaults to `localhost:4222`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flatbed::hyper::{build_router, AutoServer, ServiceContext};
use flatbed::nats::{Connector, ConnectorError};
use flatbed::telemetry::{Counter, TelemetryConfig, TelemetryError, TelemetryService};
use flatbed::{FlatbedConfig, Readiness};
use futures::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

const DEADLINE: Duration = Duration::from_secs(20);
const GATE: &str = "nats";

fn broker_addr() -> String {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "localhost:4222".to_string());
    url.trim_start_matches("nats://").to_string()
}

// ============================================================================
// A severable TCP proxy
// ============================================================================

/// A TCP forwarder the test can cut and restore, standing in for a broker
/// that goes away and comes back.
///
/// Severing both refuses new connections and drops the live ones, so the NATS
/// client sees its socket close rather than an idle link, and reconnects the
/// way it would against a restarting broker.
struct Proxy {
    addr: SocketAddr,
    severed: Arc<AtomicBool>,
    links: Arc<Mutex<Vec<JoinHandle<()>>>>,
    acceptor: JoinHandle<()>,
}

impl Proxy {
    async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a proxy port");
        let addr = listener.local_addr().expect("read the proxy port");

        let severed = Arc::new(AtomicBool::new(false));
        let links: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let acceptor = tokio::spawn({
            let severed = Arc::clone(&severed);
            let links = Arc::clone(&links);
            async move {
                while let Ok((inbound, _)) = listener.accept().await {
                    if severed.load(Ordering::SeqCst) {
                        drop(inbound);
                        continue;
                    }

                    let link = tokio::spawn(splice(inbound, upstream.clone()));
                    links.lock().expect("proxy links").push(link);
                }
            }
        });

        Self {
            addr,
            severed,
            links,
            acceptor,
        }
    }

    fn url(&self) -> String {
        self.addr.to_string()
    }

    fn sever(&self) {
        self.severed.store(true, Ordering::SeqCst);
        for link in self.links.lock().expect("proxy links").drain(..) {
            link.abort();
        }
    }

    fn heal(&self) {
        self.severed.store(false, Ordering::SeqCst);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.sever();
        self.acceptor.abort();
    }
}

async fn splice(inbound: TcpStream, upstream: String) {
    let Ok(outbound) = TcpStream::connect(&upstream).await else {
        return;
    };

    let (mut client_read, mut client_write) = inbound.into_split();
    let (mut broker_read, mut broker_write) = outbound.into_split();

    tokio::select! {
        _ = tokio::io::copy(&mut client_read, &mut broker_write) => {}
        _ = tokio::io::copy(&mut broker_read, &mut client_write) => {}
    }
}

// ============================================================================
// A telemetry service, so /readyz is mounted
// ============================================================================

/// The probes are only served when a telemetry service is configured; metrics
/// themselves are not under test here.
struct ProbesOnly;

impl TelemetryService for ProbesOnly {
    fn new(_config: TelemetryConfig) -> Arc<Self> {
        Arc::new(Self)
    }

    fn register_f64_counter(
        &self,
        _name: &str,
        _help: &str,
        _labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<f64>>, TelemetryError> {
        Err(TelemetryError::RegisterCounterError("probes only".into()))
    }

    fn register_u64_counter(
        &self,
        _name: &str,
        _help: &str,
        _labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<u64>>, TelemetryError> {
        Err(TelemetryError::RegisterCounterError("probes only".into()))
    }

    fn get_feed(&self) -> Result<String, TelemetryError> {
        Ok(String::new())
    }

    fn service_name(&self) -> String {
        "flatbed-connector-it".to_string()
    }

    fn ip_address(&self) -> String {
        "127.0.0.1".to_string()
    }
}

// ============================================================================
// Probe server
// ============================================================================

/// Serve `/readyz` for `readiness`, with the boot latch already flipped, so
/// the probe answers purely on the gates.
async fn serve_probes(readiness: Readiness) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a probe port");
    let addr = listener.local_addr().expect("read the probe port");
    drop(listener);

    let config = FlatbedConfig::new("connector-it")
        .host("127.0.0.1")
        .port(addr.port())
        .readiness(readiness)
        .with_telemetry(Arc::new(ProbesOnly));

    let (healthz_tx, healthz_rx) = watch::channel(true);
    let (ready_tx, ready_rx) = watch::channel(true);

    let service_ctx = ServiceContext {
        router: Arc::new(build_router()),
        healthz_rx,
        ready_rx,
        context: Arc::new(RwLock::new(Some(Arc::new(())))),
        config,
        static_routes: Arc::new(Vec::new()),
    };

    tokio::spawn(async move {
        let _ready_tx = ready_tx;
        AutoServer::new(addr, service_ctx, healthz_tx).serve().await
    });

    let base = format!("http://{addr}");
    wait_for("the probe server to listen", || {
        let base = base.clone();
        async move { readyz(&base).await.is_some() }
    })
    .await;

    base
}

/// The status and body of `GET /readyz`, or `None` while nothing is listening.
async fn readyz(base: &str) -> Option<(u16, String)> {
    let response = reqwest::get(format!("{base}/readyz")).await.ok()?;
    let status = response.status().as_u16();
    let body = response.text().await.ok()?;

    Some((status, body))
}

async fn wait_for<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    while started.elapsed() < DEADLINE {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("timed out after {DEADLINE:?} waiting for {what}");
}

async fn wait_for_readyz(base: &str, status: u16) -> String {
    let seen = Arc::new(Mutex::new(String::new()));

    wait_for(&format!("/readyz to answer {status}"), || {
        let seen = Arc::clone(&seen);
        async move {
            let Some((code, body)) = readyz(base).await else {
                return false;
            };
            if code != status {
                return false;
            }
            *seen.lock().expect("last body") = body;
            true
        }
    })
    .await;

    let body = seen.lock().expect("last body");
    body.clone()
}

/// Publish and receive on the client's own connection, so a healed link is
/// proven by traffic rather than by the client's own state.
async fn round_trip(client: &async_nats::Client, subject: &str) {
    let mut subscription = client
        .subscribe(subject.to_string())
        .await
        .expect("subscribe");
    client.flush().await.expect("flush the subscription");

    client
        .publish(subject.to_string(), "ping".into())
        .await
        .expect("publish");
    client.flush().await.expect("flush the publish");

    let message = tokio::time::timeout(DEADLINE, subscription.next())
        .await
        .expect("a delivery before the deadline")
        .expect("an open subscription");

    assert_eq!(&message.payload[..], b"ping");
}

// ============================================================================
// Tests
// ============================================================================

/// The acceptance case: a connection that is cut is re-established by the
/// managed connector, and `/readyz` reports 503 for exactly the interval the
/// connection is down.
#[tokio::test]
#[ignore]
async fn readyz_tracks_a_severed_and_restored_connection() {
    let proxy = Proxy::start(broker_addr()).await;
    let readiness = Readiness::new();
    let base = serve_probes(readiness.clone()).await;

    assert_eq!(
        readyz(&base).await.expect("a probe answer"),
        (200, "Ready".to_string()),
        "a server with no gates is ready on the boot latch alone"
    );

    let client = Connector::new(proxy.url())
        .name("flatbed-connector-it")
        .backoff(Duration::from_millis(50), Duration::from_millis(200))
        .readiness(readiness.gate(GATE))
        .connect_with_retry()
        .await
        .expect("connect through the proxy");

    assert_eq!(
        wait_for_readyz(&base, 200).await,
        "Ready",
        "a connected gate leaves readiness satisfied"
    );

    proxy.sever();

    let body = wait_for_readyz(&base, 503).await;
    assert!(
        body.contains(GATE),
        "the probe should name the gate holding readiness down, got {body:?}"
    );

    proxy.heal();

    assert_eq!(
        wait_for_readyz(&base, 200).await,
        "Ready",
        "readiness should return once the connection is re-established"
    );

    round_trip(&client, "flatbed.connect.restored").await;
}

/// A broker that is not listening yet is waited out rather than failing the
/// boot, which is the k8s start-order case the retry exists for.
#[tokio::test]
#[ignore]
async fn connect_with_retry_waits_for_a_late_broker() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve a port");
    let addr = listener.local_addr().expect("read the reserved port");
    drop(listener);

    let upstream = broker_addr();
    let late_broker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;

        let relay = TcpListener::bind(addr).await.expect("bind the late port");
        while let Ok((inbound, _)) = relay.accept().await {
            tokio::spawn(splice(inbound, upstream.clone()));
        }
    });

    let readiness = Readiness::new();
    let gate = readiness.gate(GATE);

    let client = Connector::new(addr.to_string())
        .backoff(Duration::from_millis(50), Duration::from_millis(200))
        .connect_deadline(DEADLINE)
        .readiness(gate.clone())
        .connect_with_retry()
        .await
        .expect("connect once the broker appears");

    assert!(gate.is_ready(), "a connected gate reports ready");
    assert!(readiness.is_ready());
    round_trip(&client, "flatbed.connect.late").await;

    late_broker.abort();
}

/// An address nothing answers on exhausts the budget and reports it, rather
/// than retrying forever behind a silent boot.
#[tokio::test]
#[ignore]
async fn connect_with_retry_gives_up_on_a_dead_address() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve a port");
    let addr = listener.local_addr().expect("read the reserved port");
    drop(listener);

    let readiness = Readiness::new();
    let gate = readiness.gate(GATE);

    let error = Connector::new(addr.to_string())
        .backoff(Duration::from_millis(20), Duration::from_millis(50))
        .connect_deadline(Duration::from_millis(400))
        .readiness(gate.clone())
        .connect_with_retry()
        .await
        .expect_err("nothing is listening on the reserved port");

    assert!(
        matches!(error, ConnectorError::Unreachable { .. }),
        "expected an unreachable error, got {error:?}"
    );
    assert!(!gate.is_ready(), "a failed connect leaves the gate closed");
    assert!(!readiness.is_ready());
}
