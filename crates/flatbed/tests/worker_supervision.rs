//! End-to-end check of the opt-in worker restart policy: a worker that fails
//! twice before settling must survive under supervision, keep the process
//! healthy while it backs off, and report its state through `/metrics`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use flatbed::{Flatbed, FlatbedConfig, FlatbedWorkerError, RestartPolicy, Worker};

static RUNS: AtomicU32 = AtomicU32::new(0);

const FAILURES_BEFORE_SETTLING: u32 = 2;

#[derive(Default)]
struct FlakyWorker;

impl Worker for FlakyWorker {
    type Context = ();

    const NAME: &'static str = "flaky-conn";
    const DESCRIPTION: Option<&'static str> = Some("fails twice, then holds");

    fn run(&self, _ctx: Arc<()>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async {
            if RUNS.fetch_add(1, Ordering::SeqCst) < FAILURES_BEFORE_SETTLING {
                return Err(FlatbedWorkerError::new("FLAKY", "injected failure"));
            }
            std::future::pending::<()>().await;
            Ok(())
        })
    }
}

flatbed::register_worker!(
    FlakyWorker,
    (),
    restart = RestartPolicy::backoff(Duration::from_millis(50), Duration::from_millis(200))
);

fn state_series(metrics: &str, state: &str) -> Option<String> {
    metrics
        .lines()
        .find(|line| {
            line.starts_with("flatbed_worker_state{")
                && line.contains(r#"worker="flaky-conn""#)
                && line.contains(&format!(r#"state="{state}""#))
        })
        .map(str::to_string)
}

#[tokio::test]
async fn restart_policy_worker_survives_failures_and_reports_state() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let telemetry_config = flatbed::TelemetryConfig::new(
        Some("supervision-test".to_string()),
        Some("127.0.0.1".to_string()),
        Some(port),
    )
    .unwrap();
    let telemetry: Arc<dyn flatbed::TelemetryService> = <flatbed::telemetry::prometheus::PrometheusTelemetryService as flatbed::TelemetryService>::new(
        telemetry_config,
    );

    let config = FlatbedConfig::new("Supervision Test")
        .host("127.0.0.1")
        .port(port)
        .with_telemetry(telemetry);

    let server = tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut metrics = String::new();
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let Ok(response) = client.get(format!("{base}/metrics")).send().await else {
            continue;
        };
        metrics = response.text().await.unwrap_or_default();
        if state_series(&metrics, "running")
            .as_deref()
            .is_some_and(|s| s.ends_with(" 1"))
            && RUNS.load(Ordering::SeqCst) == FAILURES_BEFORE_SETTLING + 1
        {
            break;
        }
    }

    assert_eq!(
        RUNS.load(Ordering::SeqCst),
        FAILURES_BEFORE_SETTLING + 1,
        "worker should have been restarted past its injected failures; metrics were:\n{metrics}"
    );

    let running = state_series(&metrics, "running")
        .unwrap_or_else(|| panic!("no running series in /metrics:\n{metrics}"));
    assert!(running.ends_with(" 1"), "expected running=1, got {running}");

    for inactive in ["backing-off", "failed"] {
        let series = state_series(&metrics, inactive)
            .unwrap_or_else(|| panic!("no {inactive} series in /metrics:\n{metrics}"));
        assert!(
            series.ends_with(" 0"),
            "expected {inactive}=0, got {series}"
        );
    }

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "backing off must not make the process unhealthy"
    );
    assert_eq!(health.text().await.unwrap(), "OK");

    server.abort();
}
