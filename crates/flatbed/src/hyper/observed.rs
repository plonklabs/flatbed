//! A streamed reply body that records how it ended.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use tracing::{info, warn};

use crate::error_reporting::{self, RouteFailure, RouteRequest, Transport};
use crate::{BoxError, FlatbedBody};

/// The code a stream's `Err` is reported with.
pub(crate) const STREAM_ERROR: &str = "STREAM_ERROR";

/// The route request a streamed reply answers.
pub(crate) struct Site {
    pub method: String,
    pub route: &'static str,
    pub path: String,
    pub request_id: String,
}

/// How a streamed body ended. Only the move out of `Open` counts, so a body
/// records exactly one end: the first of completing, failing, or being
/// dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Open,
    Completed,
    Failed,
    Disconnected,
}

impl End {
    /// Move to `to`, returning whether this was the body's end.
    fn close(&mut self, to: End) -> bool {
        if *self != End::Open {
            return false;
        }
        *self = to;
        true
    }

    fn outcome(self) -> &'static str {
        match self {
            End::Open => "open",
            End::Completed => "completed",
            End::Failed => "failed",
            End::Disconnected => "disconnected",
        }
    }
}

/// Wraps a streamed reply body, counting the bytes it sends and logging
/// `stream ended` once when it completes, fails, or is dropped unfinished.
/// A failure is also reported to the `ErrorReporter` as `STREAM_ERROR`.
pub(crate) struct Observed {
    inner: FlatbedBody,
    site: Site,
    status: u16,
    started: Instant,
    bytes: u64,
    end: End,
}

impl Observed {
    /// Wrap `inner`, the body of a reply sent with `status` to `site`.
    pub(crate) fn wrap(inner: FlatbedBody, site: Site, status: u16) -> FlatbedBody {
        Self {
            inner,
            site,
            status,
            started: Instant::now(),
            bytes: 0,
            end: End::Open,
        }
        .boxed_unsync()
    }

    fn request(&self) -> RouteRequest<'_> {
        RouteRequest {
            transport: Transport::Http,
            method: Some(&self.site.method),
            route: self.site.route,
            path: &self.site.path,
            request_id: &self.site.request_id,
        }
    }

    fn log_end(&self) {
        let route = self.site.route;
        let path = self.site.path.as_str();
        let request_id = self.site.request_id.as_str();
        let status = self.status;
        let outcome = self.end.outcome();
        let bytes = self.bytes;
        let duration_ms = self.started.elapsed().as_millis() as u64;
        if self.end == End::Completed {
            info!(
                route,
                path, request_id, status, outcome, bytes, duration_ms, "stream ended"
            );
        } else {
            warn!(
                route,
                path, request_id, status, outcome, bytes, duration_ms, "stream ended"
            );
        }
    }

    fn fail(&mut self, err: &BoxError) {
        if !self.end.close(End::Failed) {
            return;
        }
        let message = err.to_string();
        error_reporting::route_error(&RouteFailure {
            request: self.request(),
            status: self.status,
            code: Some(STREAM_ERROR),
            message: &message,
        });
        self.log_end();
    }
}

impl Body for Observed {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    self.bytes += data.len() as u64;
                }
            }
            Poll::Ready(Some(Err(err))) => self.fail(err),
            Poll::Ready(None) => {
                if self.end.close(End::Completed) {
                    self.log_end();
                }
            }
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        if self.end.close(End::Disconnected) {
            self.log_end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::End;

    fn ends(steps: &[End]) -> Vec<End> {
        let mut end = End::Open;
        steps.iter().filter(|to| end.close(**to)).copied().collect()
    }

    #[test]
    fn a_completed_body_then_dropped_ends_once_as_completed() {
        assert_eq!(ends(&[End::Completed, End::Disconnected]), [End::Completed]);
    }

    #[test]
    fn a_failed_body_then_dropped_ends_once_as_failed() {
        assert_eq!(ends(&[End::Failed, End::Disconnected]), [End::Failed]);
    }

    #[test]
    fn a_body_dropped_while_open_ends_as_disconnected() {
        assert_eq!(ends(&[End::Disconnected]), [End::Disconnected]);
    }

    #[test]
    fn a_poll_after_the_end_records_nothing() {
        assert_eq!(
            ends(&[End::Completed, End::Completed, End::Failed]),
            [End::Completed]
        );
    }
}
