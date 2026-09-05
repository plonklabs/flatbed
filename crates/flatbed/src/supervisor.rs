//! Supervision for registered background workers.
//!
//! Every worker runs inside its own task under a supervisor that observes how
//! the task ended — a returned `Err`, an unexpected `Ok(())`, or a panic — and
//! makes that outcome visible instead of letting it vanish. Kubernetes stays
//! the outer supervisor: the supervisor's job is to never leave a process
//! looking healthy with a dead worker inside it.
//!
//! Without a [`RestartPolicy`] a worker runs once. An `Err` or a panic logs,
//! marks the process unhealthy and starts graceful shutdown; an unexpected
//! `Ok(())` logs and marks the process unhealthy, leaving the surviving
//! workers running.
//!
//! With a [`RestartPolicy`] the worker is re-run after a capped, jittered
//! backoff until it exceeds the policy's restart bound, at which point it
//! takes the loud path. A run that lasts at least `max_backoff` is treated as
//! a recovery and resets the count, so the bound only ever measures
//! consecutive rapid failures.
//!
//! Once graceful shutdown has begun a worker's exit is a consequence of it,
//! so the supervisor logs the outcome and does nothing else.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use tracing::{debug, error, warn};

use crate::{WorkerFn, WorkerInfo};

/// Lifecycle state of a supervised worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerState {
    /// The worker's task is running.
    Running,
    /// The worker ended and its restart policy is waiting out a backoff.
    BackingOff,
    /// The worker ended terminally: no restart policy, or the policy's
    /// restart bound was exceeded.
    Failed,
}

impl WorkerState {
    /// Label used in the `/healthz` body and the `flatbed_worker_state` metric.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkerState::Running => "running",
            WorkerState::BackingOff => "backing-off",
            WorkerState::Failed => "failed",
        }
    }
}

#[cfg(feature = "telemetry")]
const ALL_STATES: [WorkerState; 3] = [
    WorkerState::Running,
    WorkerState::BackingOff,
    WorkerState::Failed,
];

/// Opt-in restart policy for a registered worker.
///
/// Attach one at registration to trade a pod restart for an in-process retry:
///
/// ```rust,ignore
/// flatbed::register_worker!(
///     ConnKeeper,
///     AppContext,
///     restart = flatbed::RestartPolicy::backoff(
///         std::time::Duration::from_secs(1),
///         std::time::Duration::from_secs(60),
///     )
/// );
/// ```
///
/// Without one, a worker that ends is terminal — the default flatbed
/// behaviour of crashing the pod visibly rather than retrying in place.
#[derive(Clone, Copy, Debug)]
pub struct RestartPolicy {
    min_backoff: Duration,
    max_backoff: Duration,
    max_restarts: u32,
}

impl RestartPolicy {
    /// Consecutive restarts allowed before a worker takes the loud path.
    pub const DEFAULT_MAX_RESTARTS: u32 = 10;

    /// Restart the worker with exponential backoff, doubling from
    /// `min_backoff` and capped at `max_backoff`. A `max_backoff` below
    /// `min_backoff` is raised to it.
    #[must_use]
    pub const fn backoff(min_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            min_backoff,
            max_backoff,
            max_restarts: Self::DEFAULT_MAX_RESTARTS,
        }
    }

    /// Set how many consecutive restarts are allowed before the worker
    /// escalates to the loud path.
    #[must_use]
    pub const fn max_restarts(mut self, max_restarts: u32) -> Self {
        self.max_restarts = max_restarts;
        self
    }

    fn cap(&self) -> Duration {
        if self.max_backoff > self.min_backoff {
            self.max_backoff
        } else {
            self.min_backoff
        }
    }

    fn delay(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(31);
        self.min_backoff
            .saturating_mul(1u32 << doublings)
            .min(self.cap())
    }
}

static STATES: Mutex<BTreeMap<&'static str, WorkerState>> = Mutex::new(BTreeMap::new());

#[cfg(feature = "telemetry")]
static STATE_GAUGE: std::sync::OnceLock<std::sync::Arc<dyn crate::telemetry::GaugeVec<u64>>> =
    std::sync::OnceLock::new();

/// Current state of every worker the supervisor has started, ordered by name.
#[must_use]
pub fn worker_states() -> Vec<(&'static str, WorkerState)> {
    let states = STATES.lock().unwrap_or_else(PoisonError::into_inner);
    states.iter().map(|(name, state)| (*name, *state)).collect()
}

/// Register the `flatbed_worker_state` gauge so supervisor transitions reach
/// the `/metrics` feed. Subsequent calls are ignored.
#[cfg(feature = "telemetry")]
pub(crate) fn install_state_gauge(telemetry: &std::sync::Arc<dyn crate::TelemetryService>) {
    if STATE_GAUGE.get().is_some() {
        return;
    }
    match telemetry.register_u64_gauge_vec(
        "flatbed_worker_state",
        "Supervised worker lifecycle state (1 for the state the worker is in)",
        &["worker", "state"],
    ) {
        Ok(gauge) => {
            let _ = STATE_GAUGE.set(gauge);
        }
        Err(e) => warn!(error = %e, "could not register the worker-state metric"),
    }
}

fn set_state(name: &'static str, state: WorkerState) {
    STATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name, state);
    publish_state(name, state);
}

#[cfg(feature = "telemetry")]
fn publish_state(name: &'static str, state: WorkerState) {
    let Some(gauge) = STATE_GAUGE.get() else {
        return;
    };
    for candidate in ALL_STATES {
        gauge.set(&[name, candidate.as_str()], u64::from(candidate == state));
    }
}

#[cfg(not(feature = "telemetry"))]
fn publish_state(_name: &'static str, _state: WorkerState) {}

/// How a worker's task ended.
#[derive(Debug)]
enum Exit {
    /// Returned `Ok(())` — a worker is expected to run for the process
    /// lifetime, so this is an exit, not a success.
    Completed,
    Failed(String),
    Panicked(String),
    /// The task was aborted or the runtime is shutting down.
    Cancelled,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Completed => write!(f, "returned Ok(())"),
            Exit::Failed(e) => write!(f, "returned Err: {e}"),
            Exit::Panicked(p) => write!(f, "panicked: {p}"),
            Exit::Cancelled => write!(f, "cancelled"),
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string panic payload".to_string()
}

/// Run the worker in its own task so an unwinding panic surfaces as a
/// `JoinError` instead of silently killing just that task.
async fn run_once(worker: WorkerFn, ctx: std::sync::Arc<dyn Any + Send + Sync>) -> Exit {
    match tokio::spawn(async move { worker(ctx).await }).await {
        Ok(Ok(())) => Exit::Completed,
        Ok(Err(e)) => Exit::Failed(e.to_string()),
        Err(join) if join.is_panic() => Exit::Panicked(panic_message(join.into_panic().as_ref())),
        Err(_) => Exit::Cancelled,
    }
}

/// Run a registered worker under supervision until it ends terminally.
pub(crate) async fn supervise(
    info: WorkerInfo,
    ctx: std::sync::Arc<dyn Any + Send + Sync>,
    healthz_tx: watch::Sender<bool>,
    shutdown_tx: watch::Sender<bool>,
) {
    let mut shutdown_rx = shutdown_tx.subscribe();
    let mut attempt = 0u32;

    loop {
        set_state(info.name, WorkerState::Running);
        let started = Instant::now();
        let exit = run_once(info.worker, std::sync::Arc::clone(&ctx)).await;

        if matches!(exit, Exit::Cancelled) {
            return;
        }

        // A worker whose resources are already being torn down is a
        // consequence of the shutdown, not a reason to declare the process
        // unhealthy or to restart anything.
        if *shutdown_rx.borrow() {
            debug!(worker = info.name, outcome = %exit, "worker ended during shutdown");
            return;
        }

        let Some(policy) = info.restart else {
            terminate(info.name, &exit, &healthz_tx, &shutdown_tx);
            return;
        };

        if started.elapsed() >= policy.cap() {
            attempt = 0;
        }
        attempt += 1;

        if attempt > policy.max_restarts {
            error!(
                worker = info.name,
                restarts = policy.max_restarts,
                outcome = %exit,
                "worker exhausted its restart policy",
            );
            fail(info.name, &healthz_tx, Some(&shutdown_tx));
            return;
        }

        let delay = jittered(policy.delay(attempt));
        warn!(
            worker = info.name,
            attempt,
            backoff_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            outcome = %exit,
            "worker ended; restarting after backoff",
        );
        set_state(info.name, WorkerState::BackingOff);

        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = shutdown_rx.changed() => return,
        }
    }
}

fn terminate(
    name: &'static str,
    exit: &Exit,
    healthz_tx: &watch::Sender<bool>,
    shutdown_tx: &watch::Sender<bool>,
) {
    match exit {
        Exit::Completed => {
            error!(
                worker = name,
                "worker exited; workers are expected to run for the life of the process",
            );
            fail(name, healthz_tx, None);
        }
        Exit::Failed(e) => {
            error!(worker = name, error = %e, "worker failed");
            fail(name, healthz_tx, Some(shutdown_tx));
        }
        Exit::Panicked(p) => {
            error!(worker = name, panic = %p, "worker panicked");
            fail(name, healthz_tx, Some(shutdown_tx));
        }
        Exit::Cancelled => {}
    }
}

fn fail(
    name: &'static str,
    healthz_tx: &watch::Sender<bool>,
    shutdown_tx: Option<&watch::Sender<bool>>,
) {
    set_state(name, WorkerState::Failed);
    healthz_tx.send_replace(false);
    if let Some(shutdown_tx) = shutdown_tx {
        shutdown_tx.send_replace(true);
    }
}

/// Spread the backoff over `[delay / 2, delay]`. Replicas that started at
/// different wall-clock instants draw different offsets, so a broker outage
/// they all saw at once does not produce a synchronised reconnect burst.
fn jittered(delay: Duration) -> Duration {
    let half = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX) / 2;
    if half == 0 {
        return delay;
    }
    let entropy = u64::from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos()),
    );
    Duration::from_millis(half + entropy % (half + 1))
}

#[cfg(test)]
mod tests {
    use super::{jittered, supervise, worker_states, RestartPolicy, WorkerState};
    use crate::{FlatbedWorkerError, WorkerFn, WorkerInfo};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::watch;

    static RUNS: AtomicU32 = AtomicU32::new(0);

    fn info(name: &'static str, worker: WorkerFn, restart: Option<RestartPolicy>) -> WorkerInfo {
        WorkerInfo {
            name,
            description: None,
            worker,
            restart,
        }
    }

    fn state_of(name: &str) -> Option<WorkerState> {
        worker_states()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, state)| state)
    }

    fn ok_worker(
        _ctx: Arc<dyn std::any::Any + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>>
    {
        Box::pin(async { Ok(()) })
    }

    fn err_worker(
        _ctx: Arc<dyn std::any::Any + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>>
    {
        Box::pin(async { Err(FlatbedWorkerError::new("BOOM", "worker blew up")) })
    }

    fn panicking_worker(
        _ctx: Arc<dyn std::any::Any + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>>
    {
        Box::pin(async { panic!("worker panicked on purpose") })
    }

    /// Fails until it has run three times, then parks forever.
    fn flaky_worker(
        _ctx: Arc<dyn std::any::Any + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), FlatbedWorkerError>> + Send>>
    {
        Box::pin(async {
            if RUNS.fetch_add(1, Ordering::SeqCst) < 2 {
                return Err(FlatbedWorkerError::new("FLAKY", "injected failure"));
            }
            std::future::pending::<()>().await;
            Ok(())
        })
    }

    fn channels() -> (watch::Sender<bool>, watch::Sender<bool>) {
        (watch::channel(true).0, watch::channel(false).0)
    }

    #[tokio::test]
    async fn worker_error_marks_unhealthy_and_starts_shutdown() {
        let (healthz_tx, shutdown_tx) = channels();
        supervise(
            info("err-worker", err_worker, None),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        )
        .await;

        assert!(!*healthz_tx.borrow());
        assert!(*shutdown_tx.borrow());
        assert_eq!(state_of("err-worker"), Some(WorkerState::Failed));
    }

    #[tokio::test]
    async fn worker_panic_marks_unhealthy_and_starts_shutdown() {
        let (healthz_tx, shutdown_tx) = channels();
        supervise(
            info("panicking-worker", panicking_worker, None),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        )
        .await;

        assert!(!*healthz_tx.borrow());
        assert!(*shutdown_tx.borrow());
        assert_eq!(state_of("panicking-worker"), Some(WorkerState::Failed));
    }

    #[tokio::test]
    async fn unexpected_ok_exit_marks_unhealthy_without_shutdown() {
        let (healthz_tx, shutdown_tx) = channels();
        supervise(
            info("ok-worker", ok_worker, None),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        )
        .await;

        assert!(!*healthz_tx.borrow());
        assert!(!*shutdown_tx.borrow());
        assert_eq!(state_of("ok-worker"), Some(WorkerState::Failed));
    }

    #[tokio::test]
    async fn a_worker_ending_during_shutdown_leaves_health_alone() {
        let healthz_tx = watch::channel(true).0;
        let shutdown_tx = watch::channel(true).0;

        supervise(
            info("shutdown-worker", err_worker, None),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        )
        .await;

        assert!(*healthz_tx.borrow());
        assert_eq!(state_of("shutdown-worker"), Some(WorkerState::Running));
    }

    #[tokio::test]
    async fn restart_policy_survives_injected_failures() {
        RUNS.store(0, Ordering::SeqCst);
        let (healthz_tx, shutdown_tx) = channels();
        let policy = RestartPolicy::backoff(Duration::from_millis(1), Duration::from_millis(4));

        let supervisor = tokio::spawn(supervise(
            info("flaky-worker", flaky_worker, Some(policy)),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        ));

        for _ in 0..200 {
            if state_of("flaky-worker") == Some(WorkerState::Running)
                && RUNS.load(Ordering::SeqCst) == 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(RUNS.load(Ordering::SeqCst), 3);
        assert_eq!(state_of("flaky-worker"), Some(WorkerState::Running));
        assert!(*healthz_tx.borrow(), "backing off must not flip health");
        assert!(!*shutdown_tx.borrow());
        supervisor.abort();
    }

    #[tokio::test]
    async fn exhausted_restart_policy_escalates() {
        let (healthz_tx, shutdown_tx) = channels();
        let policy = RestartPolicy::backoff(Duration::from_millis(1), Duration::from_millis(2))
            .max_restarts(2);

        supervise(
            info("doomed-worker", err_worker, Some(policy)),
            Arc::new(()),
            healthz_tx.clone(),
            shutdown_tx.clone(),
        )
        .await;

        assert!(!*healthz_tx.borrow());
        assert!(*shutdown_tx.borrow());
        assert_eq!(state_of("doomed-worker"), Some(WorkerState::Failed));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let policy = RestartPolicy::backoff(Duration::from_secs(1), Duration::from_secs(8));
        assert_eq!(policy.delay(1), Duration::from_secs(1));
        assert_eq!(policy.delay(2), Duration::from_secs(2));
        assert_eq!(policy.delay(4), Duration::from_secs(8));
        assert_eq!(policy.delay(40), Duration::from_secs(8));
    }

    #[test]
    fn backoff_cap_below_floor_is_raised_to_the_floor() {
        let policy = RestartPolicy::backoff(Duration::from_secs(5), Duration::from_secs(1));
        assert_eq!(policy.delay(1), Duration::from_secs(5));
        assert_eq!(policy.delay(9), Duration::from_secs(5));
    }

    #[test]
    fn jitter_stays_within_the_upper_half_of_the_delay() {
        let delay = Duration::from_millis(1000);
        for _ in 0..64 {
            let jittered = jittered(delay);
            assert!(jittered >= Duration::from_millis(500));
            assert!(jittered <= delay);
        }
    }
}
