//! A managed core-NATS connection.
//!
//! [`Connector`] owns the part of the connection lifecycle a service would
//! otherwise hand-roll in its boot function: retrying the first connect while
//! the broker is still coming up, loading credentials, spreading reconnect
//! attempts, and reporting the connection's state into
//! [`Readiness`](crate::Readiness) so `/readyz` tracks it.
//!
//! ```rust,ignore
//! Flatbed::run(config, |cfg| async move {
//!     let nats = flatbed::nats::Connector::new("nats://broker:4222")
//!         .credentials_file("/etc/nats/user.creds")
//!         .readiness(cfg.readiness.gate("nats"))
//!         .connect_with_retry()
//!         .await?;
//!
//!     Ok(AppContext { nats })
//! })
//! .await
//! ```
//!
//! Once the first connect succeeds the returned [`async_nats::Client`]
//! re-establishes itself on its own; the connector stays in the picture
//! through the callbacks it installed, flipping the readiness gate as the
//! connection drops and comes back.

use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::readiness::ReadinessGate;

const DEFAULT_MIN_BACKOFF: Duration = Duration::from_millis(250);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(10);
const DEFAULT_CONNECT_DEADLINE: Duration = Duration::from_secs(60);

/// A link that dies without closing the socket — a partition, a dropped
/// conntrack entry — is only noticed when pings go unanswered, so the ping
/// interval is the resolution at which the readiness gate can detect it. The
/// client tolerates a few unanswered pings before giving up on the link, so
/// detection costs a small multiple of this rather than one interval.
const PING_INTERVAL: Duration = Duration::from_secs(10);

/// Keeps the backoff shift inside the width of the type. The ramp reaches its
/// ceiling long before this many attempts.
const MAX_BACKOFF_SHIFT: u32 = 16;

enum Credentials {
    File(PathBuf),
    Inline(String),
}

/// Why a managed connection could not be established.
#[derive(Debug)]
pub enum ConnectorError {
    /// The credentials file could not be read.
    CredentialsUnreadable {
        /// The path that was configured.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// The credentials did not parse as a NATS user JWT and seed pair.
    CredentialsInvalid {
        /// The parse failure reported by the NATS client.
        source: std::io::Error,
    },
    /// No connect attempt succeeded before the retry budget ran out.
    Unreachable {
        /// The address that was dialled.
        url: String,
        /// How many attempts were made.
        attempts: u32,
        /// How long those attempts took in total.
        elapsed: Duration,
        /// The failure reported by the last attempt to finish, absent when
        /// the budget ran out while an attempt was still in flight.
        source: Option<async_nats::ConnectError>,
    },
}

impl fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CredentialsUnreadable { path, source } => {
                write!(
                    f,
                    "cannot read NATS credentials at {}: {source}",
                    path.display()
                )
            }
            Self::CredentialsInvalid { source } => {
                write!(f, "invalid NATS credentials: {source}")
            }
            Self::Unreachable {
                url,
                attempts,
                elapsed,
                source,
            } => {
                write!(
                    f,
                    "NATS at {url} unreachable after {attempts} attempts over {elapsed:.1?}"
                )?;
                match source {
                    Some(source) => write!(f, ": {source}"),
                    None => write!(f, ": no attempt finished within the budget"),
                }
            }
        }
    }
}

impl std::error::Error for ConnectorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CredentialsUnreadable { source, .. } => Some(source),
            Self::CredentialsInvalid { source } => Some(source),
            Self::Unreachable { source, .. } => source
                .as_ref()
                .map(|source| source as &dyn std::error::Error),
        }
    }
}

/// Builder for a managed core-NATS connection.
///
/// Defaults: no credentials, a 250ms-to-10s reconnect ramp, and a 60 second
/// budget for the first connect. The budget is bounded on purpose — an
/// unreachable broker should eventually surface as a failed boot rather than
/// a pod that waits forever on a typo in its configuration.
pub struct Connector {
    url: String,
    credentials: Option<Credentials>,
    client_name: Option<String>,
    min_backoff: Duration,
    max_backoff: Duration,
    deadline: Duration,
    readiness: Option<ReadinessGate>,
}

impl Connector {
    /// Start building a connection to `url`.
    ///
    /// The address takes any form [`async_nats`] accepts, including a bare
    /// `host:port`, a `nats://` URL, or a comma-separated list of servers.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            credentials: None,
            client_name: None,
            min_backoff: DEFAULT_MIN_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            deadline: DEFAULT_CONNECT_DEADLINE,
            readiness: None,
        }
    }

    /// Authenticate with the contents of a NATS credentials file.
    pub fn credentials(mut self, creds: impl Into<String>) -> Self {
        self.credentials = Some(Credentials::Inline(creds.into()));
        self
    }

    /// Authenticate with a NATS credentials file, read when the connection is
    /// made rather than now, so a missing secret mount surfaces as a connect
    /// error instead of a builder that cannot fail.
    pub fn credentials_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.credentials = Some(Credentials::File(path.into()));
        self
    }

    /// Set the client name the broker reports in `nats server report
    /// connections`.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.client_name = Some(name.into());
        self
    }

    /// Drive a readiness gate from the connection's state: ready once
    /// connected, not ready while disconnected, draining, or closed.
    pub fn readiness(mut self, gate: ReadinessGate) -> Self {
        self.readiness = Some(gate);
        self
    }

    /// Bound the delay between attempts, both for the first connect and for
    /// the reconnects the client makes afterwards. The delay doubles from
    /// `min` up to `max`.
    pub fn backoff(mut self, min: Duration, max: Duration) -> Self {
        self.min_backoff = min;
        self.max_backoff = max.max(min);
        self
    }

    /// Bound the wall time [`connect_with_retry`](Self::connect_with_retry)
    /// spends before giving up. The bound is hard: an attempt still in flight
    /// when the budget runs out is abandoned, so a URL that resolves to many
    /// addresses cannot overrun it by dialling each in turn.
    pub fn connect_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Connect, retrying with a capped, jittered backoff until the connect
    /// deadline passes. Once it returns, the client reconnects on its own for
    /// the rest of its life; the gate keeps tracking it through the callbacks
    /// installed here.
    ///
    /// A credentials problem is a configuration error, not a transient one,
    /// and fails immediately without consuming the retry budget.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError`] if the credentials cannot be read or
    /// parsed, or if no attempt succeeded within the connect deadline.
    pub async fn connect_with_retry(self) -> Result<async_nats::Client, ConnectorError> {
        let credentials = self.read_credentials()?;
        let started = Instant::now();
        let mut attempts: u32 = 0;
        let mut last_error = None;

        loop {
            let remaining = self.deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(self.unreachable(attempts, started.elapsed(), last_error));
            }

            let options = self.options(credentials.as_deref())?;
            attempts += 1;

            let Ok(attempt) = tokio::time::timeout(remaining, options.connect(&self.url)).await
            else {
                return Err(self.unreachable(attempts, started.elapsed(), last_error));
            };

            let source = match attempt {
                Ok(client) => {
                    self.mark_connected(attempts);
                    return Ok(client);
                }
                Err(source) => source,
            };

            let elapsed = started.elapsed();
            let delay = backoff(self.min_backoff, self.max_backoff, attempts);
            if elapsed + delay >= self.deadline {
                return Err(self.unreachable(attempts, elapsed, Some(source)));
            }

            warn!(
                url = %self.url,
                attempts,
                ?delay,
                error = %source,
                "NATS connect failed, retrying"
            );
            last_error = Some(source);
            tokio::time::sleep(delay).await;
        }
    }

    fn mark_connected(&self, attempts: u32) {
        if let Some(gate) = &self.readiness {
            gate.set_ready(true);
        }
        info!(url = %self.url, attempts, "connected to NATS");
    }

    fn unreachable(
        &self,
        attempts: u32,
        elapsed: Duration,
        source: Option<async_nats::ConnectError>,
    ) -> ConnectorError {
        ConnectorError::Unreachable {
            url: self.url.clone(),
            attempts,
            elapsed,
            source,
        }
    }

    fn read_credentials(&self) -> Result<Option<String>, ConnectorError> {
        match &self.credentials {
            None => Ok(None),
            Some(Credentials::Inline(creds)) => Ok(Some(creds.clone())),
            Some(Credentials::File(path)) => read_credentials_file(path).map(Some),
        }
    }

    fn options(
        &self,
        credentials: Option<&str>,
    ) -> Result<async_nats::ConnectOptions, ConnectorError> {
        let mut options = async_nats::ConnectOptions::new();

        if let Some(creds) = credentials {
            options = options
                .credentials(creds)
                .map_err(|source| ConnectorError::CredentialsInvalid { source })?;
        }

        if let Some(name) = &self.client_name {
            options = options.name(name);
        }

        let (min, max) = (self.min_backoff, self.max_backoff);
        // The client counts the dial it is about to make, so it asks for the
        // delay before the first one too, and resets the count on every
        // success. Sleeping there would put the backoff in front of every
        // connect and every recovery rather than only between retries.
        options = options
            .ping_interval(PING_INTERVAL)
            .reconnect_delay_callback(move |attempts| match attempts {
                0 | 1 => Duration::ZERO,
                retry => backoff(min, max, u32::try_from(retry - 1).unwrap_or(u32::MAX)),
            });

        let gate = self.readiness.clone();
        Ok(options.event_callback(move |event| {
            let gate = gate.clone();
            async move { on_event(&event, gate.as_ref()) }
        }))
    }
}

fn read_credentials_file(path: &Path) -> Result<String, ConnectorError> {
    std::fs::read_to_string(path).map_err(|source| ConnectorError::CredentialsUnreadable {
        path: path.to_path_buf(),
        source,
    })
}

fn on_event(event: &async_nats::Event, gate: Option<&ReadinessGate>) {
    match event {
        async_nats::Event::Connected => {
            info!(%event, "NATS connection established");
            if let Some(gate) = gate {
                gate.set_ready(true);
            }
        }
        // Draining and Closed are terminal for this client rather than a blip,
        // but they close the gate through the same path: a process on its way
        // out should leave the load balancer as promptly as one that lost its
        // link.
        async_nats::Event::Disconnected
        | async_nats::Event::Draining
        | async_nats::Event::Closed => {
            warn!(%event, "NATS connection unusable");
            if let Some(gate) = gate {
                gate.set_ready(false);
            }
        }
        other => debug!(event = %other, "NATS client event"),
    }
}

/// Half of an exponential ramp from `min` to `max`, plus a spread over the
/// other half, so replicas that lost the same broker at the same moment do
/// not retry in lockstep.
fn backoff(min: Duration, max: Duration, attempt: u32) -> Duration {
    let step = attempt.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    let capped = min.saturating_mul(1u32 << step).min(max);
    let half = capped / 2;
    half + spread(half)
}

fn spread(span: Duration) -> Duration {
    let nanos = u64::try_from(span.as_nanos()).unwrap_or(u64::MAX);
    if nanos == 0 {
        return Duration::ZERO;
    }

    Duration::from_nanos(entropy() % nanos)
}

/// Entropy has to be independent per process, so it cannot come from the
/// clock: replicas that lost the same broker read the same clock to within
/// their sync accuracy, which is exactly the correlation the spread exists to
/// break.
fn entropy() -> u64 {
    RandomState::new().build_hasher().finish()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{backoff, Connector, ConnectorError};
    use crate::readiness::Readiness;

    const MIN: Duration = Duration::from_millis(100);
    const MAX: Duration = Duration::from_millis(800);

    #[test]
    fn backoff_stays_within_the_configured_bounds() {
        for attempt in 1..=32 {
            let delay = backoff(MIN, MAX, attempt);

            assert!(delay >= MIN / 2, "attempt {attempt} waited {delay:?}");
            assert!(delay <= MAX, "attempt {attempt} waited {delay:?}");
        }
    }

    #[test]
    fn backoff_ramps_up_to_the_ceiling() {
        // Jitter spans the lower half of each step, so successive steps are
        // compared on their floor, which doubles until the cap.
        assert!(backoff(MIN, MAX, 1) < MIN);
        assert!(backoff(MIN, MAX, 4) >= MAX / 2);
        assert!(backoff(MIN, MAX, 12) >= MAX / 2);
    }

    #[test]
    fn a_zero_span_yields_no_delay() {
        assert_eq!(backoff(Duration::ZERO, Duration::ZERO, 3), Duration::ZERO);
    }

    #[tokio::test]
    async fn an_unreachable_broker_reports_the_attempts_it_made() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");

        let error = Connector::new("127.0.0.1:1")
            .backoff(Duration::from_millis(10), Duration::from_millis(20))
            .connect_deadline(Duration::from_millis(200))
            .readiness(gate)
            .connect_with_retry()
            .await
            .expect_err("nothing listens on port 1");

        let ConnectorError::Unreachable { attempts, url, .. } = error else {
            panic!("expected an unreachable error, got {error:?}");
        };
        assert_eq!(url, "127.0.0.1:1");
        assert!(attempts >= 1, "expected at least one attempt");
        assert!(
            !readiness.is_ready(),
            "a failed connect must not open the gate"
        );
    }

    #[tokio::test]
    async fn the_connect_deadline_bounds_the_wait() {
        let budget = Duration::from_millis(300);
        let started = Instant::now();

        Connector::new("127.0.0.1:1")
            .backoff(Duration::from_millis(10), Duration::from_millis(20))
            .connect_deadline(budget)
            .connect_with_retry()
            .await
            .expect_err("nothing listens on port 1");

        assert!(
            started.elapsed() < budget * 4,
            "overran the budget: {:?}",
            started.elapsed()
        );
    }

    /// A retry budget far longer than the test could tolerate, so a
    /// configuration error that consumed it would fail the test on duration
    /// rather than pass quietly.
    const UNSPENT_BUDGET: Duration = Duration::from_secs(30);

    #[tokio::test]
    async fn a_missing_credentials_file_fails_without_retrying() {
        let started = Instant::now();
        let error = Connector::new("127.0.0.1:1")
            .credentials_file("/nonexistent/flatbed/user.creds")
            .connect_deadline(UNSPENT_BUDGET)
            .connect_with_retry()
            .await
            .expect_err("the credentials file does not exist");

        assert!(
            matches!(error, ConnectorError::CredentialsUnreadable { .. }),
            "expected an unreadable-credentials error, got {error:?}"
        );
        assert!(started.elapsed() < UNSPENT_BUDGET / 2);
    }

    #[tokio::test]
    async fn malformed_credentials_fail_without_retrying() {
        let started = Instant::now();
        let error = Connector::new("127.0.0.1:1")
            .credentials("not a nats credentials file")
            .connect_deadline(UNSPENT_BUDGET)
            .connect_with_retry()
            .await
            .expect_err("the credentials do not parse");

        assert!(
            matches!(error, ConnectorError::CredentialsInvalid { .. }),
            "expected an invalid-credentials error, got {error:?}"
        );
        assert!(started.elapsed() < UNSPENT_BUDGET / 2);
    }
}
