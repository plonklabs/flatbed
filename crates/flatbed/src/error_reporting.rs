//! Server-failure reporting through an injected [`ErrorReporter`].
//!
//! A service hands flatbed a reporter with
//! [`FlatbedConfig::with_error_reporter`](crate::FlatbedConfig::with_error_reporter);
//! flatbed calls it for every server-side failure it observes:
//!
//! - a `#[route]` or `#[nats_route]` handler failing with a 5xx status, or the
//!   framework answering a matched route with one ([`ErrorReporter::route_error`]);
//! - a route handler or worker panicking ([`ErrorReporter::panic`]);
//! - a worker returning `Err`, exiting, or exhausting its restart policy
//!   ([`ErrorReporter::worker_failure`]).
//!
//! Client errors (4xx) are never reported. Reports carry no request headers,
//! query strings, or bodies.
//!
//! Wherever flatbed both reports a failure and logs it, the log event carries
//! the field `flatbed.reported = true` when a reporter received it, so a
//! tracing layer feeding the same backend can keep it out of its own events.
//!
//! # Backtraces
//!
//! When a reporter is configured, `Flatbed::run` installs a panic hook that
//! captures a backtrace and then calls the hook that was installed before it,
//! so the default stderr report and any earlier service hook keep working. A
//! hook the service installs *after* `Flatbed::run` starts replaces
//! flatbed's, and panics are then reported without a backtrace.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tracing::error;

use crate::FlatbedWorkerError;

/// Receives the server failures flatbed observes.
///
/// The report methods are called synchronously on the task that observed
/// the failure, so an implementation should hand the report off (queue,
/// channel, background transport) rather than block on I/O. `flush` runs on
/// a blocking thread.
pub trait ErrorReporter: Send + Sync + 'static {
    /// A route answered with a 5xx status. Never called for 4xx.
    fn route_error(&self, failure: &RouteFailure<'_>);

    /// A route handler or worker panicked and flatbed caught it.
    fn panic(&self, panic: &PanicReport<'_>);

    /// A worker returned `Err`, returned `Ok(())` (workers are expected to
    /// run for the life of the process), or exhausted its restart policy.
    fn worker_failure(&self, failure: &WorkerFailure<'_>);

    /// Deliver anything still buffered, waiting at most `timeout`. Called
    /// once when graceful shutdown starts; returns whether everything was
    /// delivered.
    fn flush(&self, timeout: Duration) -> bool {
        let _ = timeout;
        true
    }
}

/// The transport a route is served on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// An HTTP `#[route]`.
    Http,
    /// A core-NATS `#[nats_route]`.
    Nats,
}

/// The request a reported route failure or panic belongs to.
#[derive(Clone, Copy, Debug)]
pub struct RouteRequest<'a> {
    /// Transport the request arrived on.
    pub transport: Transport,
    /// HTTP method; `None` for NATS.
    pub method: Option<&'a str>,
    /// The declared route: the path template (`/users/{id}`) or the subject
    /// pattern (`users.{id}.get`).
    pub route: &'a str,
    /// The concrete path or subject the request arrived on.
    pub path: &'a str,
    /// The request's `x-request-id`, as echoed on the response.
    pub request_id: &'a str,
}

/// A route answered with a 5xx status.
#[derive(Clone, Copy, Debug)]
pub struct RouteFailure<'a> {
    /// The failed request.
    pub request: RouteRequest<'a>,
    /// HTTP status of the answer (the `x-error-status` of a NATS reply).
    pub status: u16,
    /// Error code put on the wire (`ERROR` for an error that set none);
    /// `None` when the answer carries no code.
    pub code: Option<&'a str>,
    /// Error message put on the wire.
    pub message: &'a str,
}

/// Where a caught panic happened.
#[derive(Clone, Copy, Debug)]
pub enum PanicSite<'a> {
    /// A route handler; the request was answered with a 500 `PANIC` error.
    Route(RouteRequest<'a>),
    /// The supervised worker with this name.
    Worker(&'a str),
}

/// A panic flatbed caught.
#[derive(Clone, Copy, Debug)]
pub struct PanicReport<'a> {
    /// Where the panic happened.
    pub site: PanicSite<'a>,
    /// The panic payload, when it was a string.
    pub message: &'a str,
    /// Backtrace captured at the panic, when flatbed's panic hook was the
    /// active one (see the module docs).
    pub backtrace: Option<&'a Backtrace>,
}

/// A supervised worker ended with a failure other than a panic, or exhausted
/// its restart policy.
#[derive(Clone, Copy, Debug)]
pub struct WorkerFailure<'a> {
    /// The worker's registered name.
    pub worker: &'a str,
    /// The error the worker returned. `None` when it returned `Ok(())`, or
    /// when a panic exhausted the restart policy — that panic is reported
    /// through [`ErrorReporter::panic`] first.
    pub error: Option<&'a FlatbedWorkerError>,
    /// The worker will not be restarted again: its restart policy ran out.
    pub restarts_exhausted: bool,
}

/// Error code a route answers a caught handler panic with.
pub(crate) const PANIC_CODE: &str = "PANIC";

static REPORTER: OnceLock<Arc<dyn ErrorReporter>> = OnceLock::new();

thread_local! {
    static LAST_BACKTRACE: Cell<Option<Backtrace>> = const { Cell::new(None) };
}

/// Make `reporter` the process-wide reporter and install the backtrace panic
/// hook. The first reporter installed in a process stays; later calls are
/// ignored.
pub(crate) fn install(reporter: &Arc<dyn ErrorReporter>) {
    if REPORTER.set(Arc::clone(reporter)).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = LAST_BACKTRACE.try_with(|slot| slot.set(Some(Backtrace::force_capture())));
        previous(info);
    }));
}

fn reporter() -> Option<&'static dyn ErrorReporter> {
    REPORTER.get().map(|reporter| &**reporter)
}

/// Report and log a 5xx answer to a route.
pub(crate) fn route_error(failure: &RouteFailure<'_>) {
    let reporter = reporter();
    if let Some(reporter) = reporter {
        reporter.route_error(failure);
    }
    let request = &failure.request;
    error!(
        transport = ?request.transport,
        method = request.method,
        route = request.route,
        path = request.path,
        request_id = request.request_id,
        status = failure.status,
        code = failure.code,
        message = failure.message,
        flatbed.reported = reporter.is_some(),
        "route failed",
    );
}

/// Report and log a panic caught in a route handler.
pub(crate) fn route_panic(request: RouteRequest<'_>, panic: &CaughtPanic) {
    let reported = report_panic(PanicSite::Route(request), panic);
    error!(
        transport = ?request.transport,
        method = request.method,
        route = request.route,
        path = request.path,
        request_id = request.request_id,
        panic = %panic.message,
        flatbed.reported = reported,
        "route handler panicked; answering with a PANIC error",
    );
}

/// Report a caught panic; returns whether a reporter received it.
pub(crate) fn report_panic(site: PanicSite<'_>, panic: &CaughtPanic) -> bool {
    let Some(reporter) = reporter() else {
        return false;
    };
    reporter.panic(&PanicReport {
        site,
        message: &panic.message,
        backtrace: panic.backtrace.as_ref(),
    });
    true
}

/// Report a worker failure; returns whether a reporter received it.
pub(crate) fn report_worker_failure(failure: &WorkerFailure<'_>) -> bool {
    let Some(reporter) = reporter() else {
        return false;
    };
    reporter.worker_failure(failure);
    true
}

/// Flush the installed reporter, waiting at most `timeout` whether or not
/// the reporter honours it.
pub(crate) async fn flush(timeout: Duration) {
    let Some(reporter) = REPORTER.get() else {
        return;
    };
    flush_reporter(Arc::clone(reporter), timeout).await;
}

async fn flush_reporter(reporter: Arc<dyn ErrorReporter>, timeout: Duration) {
    let flushing = tokio::task::spawn_blocking(move || reporter.flush(timeout));
    let _ = tokio::time::timeout(timeout, flushing).await;
}

/// Report a 5xx error returned by a `#[route]` handler. Called by the code
/// `#[route]` generates; not part of the public API.
#[doc(hidden)]
pub fn __report_http_route_error(
    method: &str,
    route: &str,
    path: &str,
    request_id: &str,
    status: http::StatusCode,
    code: &str,
    message: &str,
) {
    if !status.is_server_error() {
        return;
    }
    route_error(&RouteFailure {
        request: RouteRequest {
            transport: Transport::Http,
            method: Some(method),
            route,
            path,
            request_id,
        },
        status: status.as_u16(),
        code: Some(code),
        message,
    });
}

/// A panic caught by [`CatchPanic`].
#[derive(Debug)]
pub(crate) struct CaughtPanic {
    pub(crate) message: String,
    pub(crate) backtrace: Option<Backtrace>,
}

impl std::fmt::Display for CaughtPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Resolve to `Err` instead of unwinding when `future` panics.
pub(crate) fn catch_panic<F: Future + Unpin>(future: F) -> CatchPanic<F> {
    CatchPanic(future)
}

/// Future returned by [`catch_panic`].
///
/// The panic hook stores the backtrace in a thread-local, so it is taken
/// inside the same `poll` that caught the unwind: a later poll may run on a
/// different worker thread.
pub(crate) struct CatchPanic<F>(F);

impl<F: Future + Unpin> Future for CatchPanic<F> {
    type Output = Result<F::Output, CaughtPanic>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.0;
        match std::panic::catch_unwind(AssertUnwindSafe(|| Pin::new(inner).poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(payload) => Poll::Ready(Err(CaughtPanic {
                message: panic_message(payload.as_ref()),
                backtrace: LAST_BACKTRACE.try_with(Cell::take).ok().flatten(),
            })),
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string panic payload".to_string()
}

#[cfg(test)]
mod tests {
    use super::{flush_reporter, ErrorReporter, PanicReport, RouteFailure, WorkerFailure};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct SlowFlush;

    impl ErrorReporter for SlowFlush {
        fn route_error(&self, _: &RouteFailure<'_>) {}
        fn panic(&self, _: &PanicReport<'_>) {}
        fn worker_failure(&self, _: &WorkerFailure<'_>) {}
        fn flush(&self, _timeout: Duration) -> bool {
            std::thread::sleep(Duration::from_secs(1));
            true
        }
    }

    #[tokio::test]
    async fn flush_is_bounded_even_when_the_reporter_ignores_its_timeout() {
        let started = Instant::now();
        flush_reporter(Arc::new(SlowFlush), Duration::from_millis(100)).await;
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
