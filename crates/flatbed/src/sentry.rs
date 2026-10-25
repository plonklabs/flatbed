//! An [`ErrorReporter`] that sends flatbed's server failures to Sentry, or to
//! any server speaking Sentry's ingestion protocol (GlitchTip, Bugsink).
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use flatbed::sentry::SentryReporter;
//! use flatbed::FlatbedConfig;
//! use tracing_subscriber::layer::SubscriberExt;
//! use tracing_subscriber::util::SubscriberInitExt;
//!
//! let reporter = Arc::new(SentryReporter::from_env());
//! tracing_subscriber::registry()
//!     .with(tracing_subscriber::fmt::layer())
//!     .with(reporter.layer())
//!     .init();
//! let config = FlatbedConfig::new("My API").with_error_reporter(reporter);
//! ```
//!
//! Events carry the route template, method, transport, status, error code,
//! request id, or worker name as tags, the concrete path or subject as extra
//! data, and the panic backtrace as the stacktrace. Request headers, query
//! strings, bodies, and client IPs are never sent.

use std::backtrace::Backtrace;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ::sentry::integrations::backtrace::ProcessStacktraceIntegration;
use ::sentry::integrations::contexts::ContextIntegration;
use ::sentry::integrations::tracing::{
    breadcrumb_from_event, event_from_event, log_from_event, EventMapping, SentryLayer,
};
use ::sentry::protocol::{Event, Exception, Frame, Level, Mechanism, Stacktrace, Value};
use ::sentry::{ClientInitGuard, ClientOptions, Hub, Integration};
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::error_reporting::PANIC_CODE;
use crate::{
    ErrorReporter, PanicReport, PanicSite, RouteFailure, RouteRequest, Transport, WorkerFailure,
};

const REPORTED_FIELD: &str = "flatbed.reported";
const PANIC_TYPE: &str = "panic";

/// Sends flatbed's server failures to the Sentry client configured from the
/// environment.
///
/// Without a usable `SENTRY_DSN` the reporter does nothing: no client is
/// created, so no thread is started and nothing touches the network.
pub struct SentryReporter {
    _client: Option<ClientInitGuard>,
    logs: bool,
}

impl SentryReporter {
    /// Initialize the Sentry client from `SENTRY_DSN`, `SENTRY_ENVIRONMENT`,
    /// and `SENTRY_RELEASE`, binding it to the calling thread's hub. Call it
    /// first thing in `main`, before anything else touches Sentry (and so
    /// before a tracing subscriber exists: an unparsable DSN is reported on
    /// stderr), and keep the reporter alive for the life of the process:
    /// dropping it flushes and closes the client.
    pub fn from_env() -> Self {
        if std::env::var("SENTRY_DSN").is_ok_and(|dsn| dsn.parse::<::sentry::types::Dsn>().is_err())
        {
            eprintln!("flatbed: SENTRY_DSN is not a valid DSN; Sentry reporting is off");
        }
        Self::from_options(client_options())
    }

    fn from_options(options: ClientOptions) -> Self {
        let options = ::sentry::apply_defaults(options);
        let _client = options.dsn.is_some().then(|| ::sentry::init(options));
        Self {
            _client,
            logs: false,
        }
    }

    /// Also send `error!`, `warn!`, and `info!` events through [`Self::layer`]
    /// as Sentry Logs.
    pub fn with_logs(mut self) -> Self {
        self.logs = true;
        self
    }

    /// A tracing layer feeding the same Sentry client: `error!` events
    /// become Sentry events, `warn!` and `info!` events become breadcrumbs,
    /// and an event logged with `flatbed.reported = true` becomes a
    /// breadcrumb only, since the reporter already sent it.
    pub fn layer<S>(&self) -> SentryLayer<S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let logs = self.logs;
        ::sentry::integrations::tracing::layer().event_mapper(move |event, _: Context<'_, S>| {
            let no_span = None::<&Context<'_, S>>;
            let level = *event.metadata().level();
            if level > tracing::Level::INFO {
                return EventMapping::Ignore;
            }
            let mapping = if level == tracing::Level::ERROR && !reported(event) {
                EventMapping::Event(Box::new(event_from_event(event, no_span)))
            } else {
                EventMapping::Breadcrumb(breadcrumb_from_event(event, no_span))
            };
            if !logs {
                return mapping;
            }
            let log = EventMapping::Log(log_from_event(event, no_span));
            EventMapping::Combined(vec![mapping, log].into())
        })
    }
}

fn client_options() -> ClientOptions {
    let integrations: Vec<Arc<dyn Integration>> = vec![
        Arc::new(ContextIntegration::default()),
        Arc::new(ProcessStacktraceIntegration),
    ];
    let mut options = ClientOptions::default();
    options.send_default_pii = false;
    options.default_integrations = false;
    options.integrations = integrations;
    options
}

fn reported(event: &tracing::Event<'_>) -> bool {
    struct Reported(bool);
    impl Visit for Reported {
        fn record_bool(&mut self, field: &Field, value: bool) {
            if field.name() == REPORTED_FIELD {
                self.0 = value;
            }
        }
        fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    }
    let mut visitor = Reported(false);
    event.record(&mut visitor);
    visitor.0
}

impl ErrorReporter for SentryReporter {
    fn route_error(&self, failure: &RouteFailure<'_>) {
        let mut event = route_event(&failure.request);
        event.exception = vec![Exception {
            ty: failure
                .code
                .map_or_else(|| format!("HTTP {}", failure.status), str::to_owned),
            value: Some(failure.message.to_owned()),
            ..Exception::default()
        }]
        .into();
        set_tag(&mut event, "status", failure.status.to_string());
        if let Some(code) = failure.code {
            set_tag(&mut event, "code", code);
        }
        capture(event);
    }

    fn panic(&self, panic: &PanicReport<'_>) {
        let mut event = match panic.site {
            PanicSite::Route(request) => {
                let mut event = route_event(&request);
                set_tag(&mut event, "status", "500");
                set_tag(&mut event, "code", PANIC_CODE);
                event
            }
            PanicSite::Worker(worker) => worker_event(worker),
        };
        event.exception = vec![Exception {
            ty: PANIC_TYPE.to_owned(),
            value: Some(panic.message.to_owned()),
            stacktrace: panic.backtrace.and_then(stacktrace),
            mechanism: Some(Mechanism {
                ty: PANIC_TYPE.to_owned(),
                handled: Some(true),
                ..Mechanism::default()
            }),
            ..Exception::default()
        }]
        .into();
        capture(event);
    }

    fn worker_failure(&self, failure: &WorkerFailure<'_>) {
        let mut event = match failure.error {
            Some(error) => {
                let mut event = ::sentry::event_from_error(error);
                set_tag(&mut event, "code", error.code.clone());
                event
            }
            None if failure.restarts_exhausted => {
                message_event("worker exhausted its restart policy")
            }
            None => message_event("worker exited"),
        };
        let worker = worker_event(failure.worker);
        event.transaction = worker.transaction;
        event.tags.extend(worker.tags);
        set_tag(
            &mut event,
            "restarts_exhausted",
            failure.restarts_exhausted.to_string(),
        );
        capture(event);
    }

    fn flush(&self, timeout: Duration) -> bool {
        Hub::current()
            .client()
            .is_none_or(|client| client.flush(Some(timeout)))
    }
}

fn route_event(request: &RouteRequest<'_>) -> Event<'static> {
    let transport = match request.transport {
        Transport::Http => "http",
        Transport::Nats => "nats",
    };
    let mut event = Event {
        level: Level::Error,
        transaction: Some(match request.method {
            Some(method) => format!("{method} {}", request.route),
            None => request.route.to_owned(),
        }),
        ..Event::default()
    };
    set_tag(&mut event, "transport", transport);
    set_tag(&mut event, "route", request.route);
    set_tag(&mut event, "request_id", request.request_id);
    if let Some(method) = request.method {
        set_tag(&mut event, "method", method);
    }
    event
        .extra
        .insert("path".to_owned(), Value::from(request.path));
    event
}

fn worker_event(worker: &str) -> Event<'static> {
    let mut event = Event {
        level: Level::Error,
        transaction: Some(worker.to_owned()),
        ..Event::default()
    };
    set_tag(&mut event, "worker", worker);
    event
}

fn message_event(message: &str) -> Event<'static> {
    Event {
        level: Level::Error,
        message: Some(message.to_owned()),
        ..Event::default()
    }
}

fn set_tag(event: &mut Event<'static>, key: &str, value: impl Into<String>) {
    event.tags.insert(key.to_owned(), value.into());
}

/// Tags travel on the event itself, so concurrent reports never share scope
/// state.
fn capture(event: Event<'static>) {
    Hub::current().capture_event(event);
}

/// Library frame locations: the toolchain's std sources and crates fetched by
/// cargo. Sentry's own in-app heuristic goes by function prefix, which misses
/// `<[T] as Trait>` impls and every non-std dependency.
const LIBRARY_PATHS: &[&str] = &[
    "/rustc/",
    "/rustlib/src/rust/",
    "/.cargo/registry/",
    "/.cargo/git/",
];

fn stacktrace(backtrace: &Backtrace) -> Option<Stacktrace> {
    stacktrace_from_str(&backtrace.to_string())
}

/// Parse std's `Backtrace` display format: per symbol, `N: name` (no index for
/// an inlined symbol) optionally followed by `at path:line[:col]`, newest
/// first. Frames up to std's `__rust_end_short_backtrace` marker are the
/// panic machinery and are dropped.
fn stacktrace_from_str(backtrace: &str) -> Option<Stacktrace> {
    let mut frames: Vec<Frame> = Vec::new();
    for line in backtrace.lines().map(str::trim) {
        if let Some(location) = line.strip_prefix("at ") {
            if let Some(frame) = frames.last_mut() {
                set_location(frame, location);
            }
            continue;
        }
        let symbol = line
            .split_once(": ")
            .filter(|(index, _)| index.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or(line, |(_, symbol)| symbol);
        frames.push(Frame {
            function: Some(symbol.to_owned()),
            ..Frame::default()
        });
    }
    let machinery = frames
        .iter()
        .position(|frame| {
            frame
                .function
                .as_deref()
                .is_some_and(|function| function.contains("__rust_end_short_backtrace"))
        })
        .map_or(0, |marker| marker + 1);
    frames.drain(..machinery);
    frames.reverse();
    (!frames.is_empty()).then(|| Stacktrace {
        frames,
        ..Stacktrace::default()
    })
}

fn set_location(frame: &mut Frame, location: &str) {
    let (path, lineno, colno) = match trailing_number(location) {
        Some((rest, last)) => match trailing_number(rest) {
            Some((path, line)) => (path, Some(line), Some(last)),
            None => (rest, Some(last), None),
        },
        None => (location, None, None),
    };
    if LIBRARY_PATHS.iter().any(|library| path.contains(library)) {
        frame.in_app = Some(false);
    }
    frame.filename = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    frame.abs_path = Some(path.to_owned());
    frame.lineno = lineno;
    frame.colno = colno;
}

fn trailing_number(text: &str) -> Option<(&str, u64)> {
    let (head, tail) = text.rsplit_once(':')?;
    Some((head, tail.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use std::backtrace::Backtrace;

    use ::sentry::protocol::Event;
    use ::sentry::protocol::{EnvelopeItem, ItemContainer};
    use ::sentry::test::{with_captured_envelopes, with_captured_events};
    use tracing_subscriber::layer::SubscriberExt;

    use super::{client_options, stacktrace_from_str, SentryReporter};
    use crate::{
        ErrorReporter, FlatbedWorkerError, PanicReport, PanicSite, RouteFailure, RouteRequest,
        Transport, WorkerFailure,
    };

    fn reporter() -> SentryReporter {
        SentryReporter {
            _client: None,
            logs: false,
        }
    }

    fn request() -> RouteRequest<'static> {
        RouteRequest {
            transport: Transport::Http,
            method: Some("GET"),
            route: "/users/{id}",
            path: "/users/7",
            request_id: "req-1",
        }
    }

    fn tag<'a>(event: &'a Event<'static>, key: &str) -> Option<&'a str> {
        event.tags.get(key).map(String::as_str)
    }

    #[inline(never)]
    fn failing_site() -> Backtrace {
        Backtrace::force_capture()
    }

    #[test]
    fn route_error_is_one_event_tagged_with_the_request() {
        let events = with_captured_events(|| {
            reporter().route_error(&RouteFailure {
                request: request(),
                status: 503,
                code: Some("DB_DOWN"),
                message: "database unavailable",
            });
        });

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(tag(event, "transport"), Some("http"));
        assert_eq!(tag(event, "method"), Some("GET"));
        assert_eq!(tag(event, "route"), Some("/users/{id}"));
        assert_eq!(tag(event, "status"), Some("503"));
        assert_eq!(tag(event, "code"), Some("DB_DOWN"));
        assert_eq!(tag(event, "request_id"), Some("req-1"));
        assert_eq!(event.transaction.as_deref(), Some("GET /users/{id}"));
        assert_eq!(event.exception[0].ty, "DB_DOWN");
        assert_eq!(
            event.exception[0].value.as_deref(),
            Some("database unavailable")
        );
    }

    #[test]
    fn tags_stay_on_their_own_event() {
        let events = with_captured_events(|| {
            reporter().panic(&PanicReport {
                site: PanicSite::Worker("indexer"),
                message: "boom",
                backtrace: None,
            });
            reporter().route_error(&RouteFailure {
                request: request(),
                status: 500,
                code: None,
                message: "oops",
            });
            ::sentry::capture_message("unrelated", ::sentry::Level::Error);
        });

        assert_eq!(events.len(), 3);
        assert_eq!(tag(&events[0], "worker"), Some("indexer"));
        assert_eq!(tag(&events[1], "worker"), None);
        assert_eq!(tag(&events[1], "code"), None);
        assert_eq!(events[1].exception[0].ty, "HTTP 500");
        assert!(events[2].tags.is_empty());
    }

    #[test]
    fn panic_backtrace_becomes_the_exception_stacktrace() {
        let backtrace = failing_site();
        let events = with_captured_events(|| {
            reporter().panic(&PanicReport {
                site: PanicSite::Route(request()),
                message: "index out of bounds",
                backtrace: Some(&backtrace),
            });
        });

        let exception = &events[0].exception[0];
        assert_eq!(exception.ty, "panic");
        assert_eq!(exception.value.as_deref(), Some("index out of bounds"));
        let frames = &exception.stacktrace.as_ref().expect("stacktrace").frames;
        assert!(frames.iter().any(|frame| frame
            .function
            .as_deref()
            .is_some_and(|function| function.contains("failing_site"))));
        assert_eq!(tag(&events[0], "route"), Some("/users/{id}"));
        assert_eq!(tag(&events[0], "status"), Some("500"));
        assert_eq!(tag(&events[0], "code"), Some("PANIC"));
    }

    #[test]
    fn stacktrace_parses_std_backtraces_without_the_panic_machinery() {
        let backtrace = "   0: std::backtrace::Backtrace::force_capture
             at /rustc/abc/library/std/src/backtrace.rs:310:9
   1: std::panicking::panic_with_hook
             at /rustc/abc/library/std/src/panicking.rs:841:13
   2: std::sys::backtrace::__rust_end_short_backtrace::<std::panicking::begin_panic_handler::{closure#0}, !>
             at /rustc/abc/library/std/src/sys/backtrace.rs:168:18
   3: __rustc::rust_begin_unwind
             at /rustc/abc/library/std/src/panicking.rs:679:5
   4: <alloc::vec::Vec<u32> as core::ops::index::Index<usize>>::index
             at /Users/me/.rustup/toolchains/stable/lib/rustlib/src/rust/library/alloc/src/vec/mod.rs:3934:9
   5: my_service::handler::{closure#0}
             at ./src/main.rs:12:5
      my_service::inlined_helper
             at ./src/main.rs:20
   6: <core::pin::Pin<Box<dyn Future<Output = Result<(), ()>>>> as Future>::poll
             at /Users/me/.cargo/registry/src/index.crates.io-1/flatbed-0.0.7/src/hyper/service.rs:126:59
   7: main
";
        let stacktrace = stacktrace_from_str(backtrace).expect("stacktrace");
        let frames: Vec<_> = stacktrace
            .frames
            .iter()
            .map(|frame| {
                (
                    frame.function.as_deref().unwrap_or_default(),
                    frame.filename.as_deref(),
                    frame.lineno,
                    frame.colno,
                    frame.in_app,
                )
            })
            .collect();
        assert_eq!(
            frames,
            [
                ("main", None, None, None, None),
                (
                    "<core::pin::Pin<Box<dyn Future<Output = Result<(), ()>>>> as Future>::poll",
                    Some("service.rs"),
                    Some(126),
                    Some(59),
                    Some(false),
                ),
                (
                    "my_service::inlined_helper",
                    Some("main.rs"),
                    Some(20),
                    None,
                    None
                ),
                (
                    "my_service::handler::{closure#0}",
                    Some("main.rs"),
                    Some(12),
                    Some(5),
                    None
                ),
                (
                    "<alloc::vec::Vec<u32> as core::ops::index::Index<usize>>::index",
                    Some("mod.rs"),
                    Some(3934),
                    Some(9),
                    Some(false),
                ),
                (
                    "__rustc::rust_begin_unwind",
                    Some("panicking.rs"),
                    Some(679),
                    Some(5),
                    Some(false)
                ),
            ]
        );
    }

    #[test]
    fn worker_error_is_the_typed_exception() {
        let error = FlatbedWorkerError::new("CACHE_ERROR", "cache unreachable");
        let events = with_captured_events(|| {
            reporter().worker_failure(&WorkerFailure {
                worker: "cache-warmer",
                error: Some(&error),
                restarts_exhausted: true,
            });
        });

        let event = &events[0];
        assert_eq!(event.exception[0].ty, "FlatbedWorkerError");
        assert_eq!(
            event.exception[0].value.as_deref(),
            Some("[CACHE_ERROR] cache unreachable")
        );
        assert_eq!(tag(event, "worker"), Some("cache-warmer"));
        assert_eq!(tag(event, "code"), Some("CACHE_ERROR"));
        assert_eq!(tag(event, "restarts_exhausted"), Some("true"));
    }

    #[test]
    fn worker_exit_without_an_error_is_a_message_event() {
        let events = with_captured_events(|| {
            for restarts_exhausted in [false, true] {
                reporter().worker_failure(&WorkerFailure {
                    worker: "poller",
                    error: None,
                    restarts_exhausted,
                });
            }
        });

        let summary: Vec<_> = events
            .iter()
            .map(|event| {
                (
                    event.message.as_deref(),
                    tag(event, "worker"),
                    tag(event, "restarts_exhausted"),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some("worker exited"), Some("poller"), Some("false")),
                (
                    Some("worker exhausted its restart policy"),
                    Some("poller"),
                    Some("true")
                ),
            ]
        );
    }

    #[test]
    fn layer_sends_unreported_errors_and_keeps_reported_ones_as_breadcrumbs() {
        let reporter = reporter();
        let events = with_captured_events(|| {
            let subscriber = tracing_subscriber::registry().with(reporter.layer());
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!("booted");
                tracing::error!(flatbed.reported = true, "route failed");
                tracing::error!(flatbed.reported = false, "route failed without a reporter");
                tracing::warn!("slow query");
                tracing::error!("cache miss storm");
                tracing::debug!("ignored");
            });
        });

        let messages: Vec<_> = events
            .iter()
            .map(|event| event.message.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(
            messages,
            ["route failed without a reporter", "cache miss storm"]
        );
        let breadcrumbs: Vec<_> = events[1]
            .breadcrumbs
            .iter()
            .map(|crumb| crumb.message.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(breadcrumbs, ["booted", "route failed", "slow query"]);
    }

    #[test]
    fn with_logs_also_sends_sentry_logs() {
        let log_items = |reporter: SentryReporter| {
            with_captured_envelopes(|| {
                let subscriber = tracing_subscriber::registry().with(reporter.layer());
                tracing::subscriber::with_default(subscriber, || tracing::warn!("slow query"));
            })
            .iter()
            .flat_map(|envelope| envelope.items())
            .filter(|item| matches!(item, EnvelopeItem::ItemContainer(ItemContainer::Logs(_))))
            .count()
        };

        assert_eq!(log_items(reporter()), 0);
        assert_eq!(log_items(reporter().with_logs()), 1);
    }

    #[test]
    fn without_a_dsn_nothing_is_initialized() {
        let reporter = SentryReporter::from_options(client_options());
        assert!(reporter._client.is_none());
        assert!(::sentry::Hub::current().client().is_none());
        reporter.route_error(&RouteFailure {
            request: request(),
            status: 500,
            code: None,
            message: "dropped",
        });
        assert!(reporter.flush(std::time::Duration::from_millis(10)));
    }

    #[test]
    fn sentry_panic_integration_and_pii_stay_off() {
        let options = client_options();
        assert!(!options.send_default_pii);
        assert!(!options.default_integrations);
        assert!(options
            .integrations
            .iter()
            .all(|integration| integration.name() != "panic"));
    }
}
