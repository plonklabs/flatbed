//! End-to-end check of worker error reporting through a real `Flatbed::run`:
//! a worker that panics, is restarted, and then exhausts its restart policy
//! with a typed error must reach the reporter as a panic with a backtrace,
//! then the typed failure, then a flush as graceful shutdown starts.
//!
//! The worker's failure shuts the server down, so this binary runs a single
//! server and a single test.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flatbed::{
    ErrorReporter, Flatbed, FlatbedConfig, FlatbedWorkerError, PanicReport, PanicSite,
    RestartPolicy, RouteFailure, Worker, WorkerFailure,
};

static RUNS: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
struct DoomedWorker;

impl Worker for DoomedWorker {
    type Context = ();

    const NAME: &'static str = "doomed";
    const RESTART: Option<RestartPolicy> = Some(
        RestartPolicy::backoff(Duration::from_millis(10), Duration::from_secs(1)).max_restarts(1),
    );

    fn run(&self, _ctx: Arc<()>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async {
            if RUNS.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("worker panicked on purpose");
            }
            Err(FlatbedWorkerError::new("TYPED", "typed failure"))
        })
    }
}

flatbed::register_worker!(DoomedWorker, ());

#[derive(Debug, PartialEq)]
enum Record {
    Panic {
        worker: String,
        message: String,
        backtrace: bool,
    },
    WorkerFailure {
        worker: String,
        error: Option<(String, String)>,
        restarts_exhausted: bool,
    },
    Flush(Duration),
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Record>>);

impl ErrorReporter for Recorder {
    fn route_error(&self, _failure: &RouteFailure<'_>) {}

    fn panic(&self, panic: &PanicReport<'_>) {
        let PanicSite::Worker(worker) = panic.site else {
            return;
        };
        self.0.lock().unwrap().push(Record::Panic {
            worker: worker.to_string(),
            message: panic.message.to_string(),
            backtrace: panic.backtrace.is_some(),
        });
    }

    fn worker_failure(&self, failure: &WorkerFailure<'_>) {
        self.0.lock().unwrap().push(Record::WorkerFailure {
            worker: failure.worker.to_string(),
            error: failure.error.map(|e| (e.code.clone(), e.message.clone())),
            restarts_exhausted: failure.restarts_exhausted,
        });
    }

    fn flush(&self, timeout: Duration) -> bool {
        self.0.lock().unwrap().push(Record::Flush(timeout));
        true
    }
}

#[tokio::test]
async fn worker_panic_and_typed_failure_are_reported_then_flushed_at_shutdown() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let recorder = Arc::new(Recorder::default());
    let reporter: Arc<dyn ErrorReporter> = recorder.clone();
    let config = FlatbedConfig::new("report-worker")
        .host("127.0.0.1")
        .port(port)
        .with_error_reporter(reporter);
    tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(()) }).await });

    for _ in 0..250 {
        let flushed = recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|record| matches!(record, Record::Flush(_)));
        if flushed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![
            Record::Panic {
                worker: "doomed".to_string(),
                message: "worker panicked on purpose".to_string(),
                backtrace: true,
            },
            Record::WorkerFailure {
                worker: "doomed".to_string(),
                error: Some(("TYPED".to_string(), "typed failure".to_string())),
                restarts_exhausted: true,
            },
            Record::Flush(Duration::from_secs(2)),
        ]
    );
}
