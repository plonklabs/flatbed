//! End-to-end check of `Worker::enabled`: a worker whose check rejects the
//! application context is never started, leaves the process healthy, and is
//! reported as `disabled` in `/healthz` and `flatbed_worker_state`, while a
//! worker the same context enables runs as usual.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use flatbed::{Flatbed, FlatbedConfig, FlatbedWorkerError, Worker};

#[derive(Clone)]
struct AppContext {
    provider: &'static str,
}

static K3S_ONLY_STARTS: AtomicU32 = AtomicU32::new(0);

/// Returns at once when started, which marks the process unhealthy, so a
/// green `/healthz` proves it never ran.
#[derive(Default)]
struct K3sOnly;

impl Worker for K3sOnly {
    type Context = AppContext;

    const NAME: &'static str = "k3s-only";

    fn enabled(&self, ctx: &AppContext) -> bool {
        ctx.provider == "k3s"
    }

    fn run(&self, _ctx: Arc<AppContext>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async {
            K3S_ONLY_STARTS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

flatbed::register_worker!(K3sOnly, AppContext);

#[derive(Default)]
struct GkeOnly;

impl Worker for GkeOnly {
    type Context = AppContext;

    const NAME: &'static str = "gke-only";

    fn enabled(&self, ctx: &AppContext) -> bool {
        ctx.provider == "gke"
    }

    fn run(&self, _ctx: Arc<AppContext>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async {
            std::future::pending::<()>().await;
            Ok(())
        })
    }
}

flatbed::register_worker!(GkeOnly, AppContext);

fn state_value(metrics: &str, worker: &str, state: &str) -> Option<String> {
    metrics
        .lines()
        .find(|line| {
            line.starts_with("flatbed_worker_state{")
                && line.contains(&format!(r#"worker="{worker}""#))
                && line.contains(&format!(r#"state="{state}""#))
        })
        .and_then(|line| line.rsplit(' ').next())
        .map(str::to_string)
}

#[tokio::test]
async fn disabled_worker_is_not_started_and_reported_as_disabled() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let telemetry_config = flatbed::TelemetryConfig::new(
        Some("worker-disabled-test".to_string()),
        Some("127.0.0.1".to_string()),
        Some(port),
    )
    .unwrap();
    let telemetry: Arc<dyn flatbed::TelemetryService> = <flatbed::telemetry::prometheus::PrometheusTelemetryService as flatbed::TelemetryService>::new(
        telemetry_config,
    );

    let config = FlatbedConfig::new("Worker Disabled Test")
        .host("127.0.0.1")
        .port(port)
        .with_telemetry(telemetry);

    let server = tokio::spawn(async move {
        Flatbed::run(config, |_| async { Ok(AppContext { provider: "gke" }) }).await
    });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut metrics = String::new();
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let Ok(response) = client.get(format!("{base}/metrics")).send().await else {
            continue;
        };
        metrics = response.text().await.unwrap_or_default();
        if state_value(&metrics, "k3s-only", "disabled").is_some()
            && state_value(&metrics, "gke-only", "running").is_some()
        {
            break;
        }
    }

    assert_eq!(
        K3S_ONLY_STARTS.load(Ordering::SeqCst),
        0,
        "a disabled worker must not be started"
    );

    for (worker, state, expected) in [
        ("k3s-only", "disabled", "1"),
        ("k3s-only", "running", "0"),
        ("k3s-only", "failed", "0"),
        ("gke-only", "running", "1"),
        ("gke-only", "disabled", "0"),
    ] {
        assert_eq!(
            state_value(&metrics, worker, state).as_deref(),
            Some(expected),
            "{worker} {state}; metrics were:\n{metrics}"
        );
    }

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "a disabled worker must not make the process unhealthy"
    );
    assert_eq!(health.text().await.unwrap(), "OK\nk3s-only: disabled");

    server.abort();
}
