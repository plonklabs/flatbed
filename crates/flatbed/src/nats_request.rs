//! Typed core-NATS requests — the client side of `#[nats_route]`.
//!
//! [`NatsRequestExt::typed_request`] encodes a body, publishes it on a
//! subject, waits for the reply, and decodes it into the response type the
//! call site binds:
//!
//! ```rust,ignore
//! use flatbed::NatsRequestExt;
//!
//! let status: SatelliteStatus = ctx.nats
//!     .typed_request("plonk.satellite.x07.call.status", &StatusQuery::default())
//!     .timeout(Duration::from_secs(5))
//!     .await?;
//! ```
//!
//! The method is `typed_request` rather than `request` because
//! `async_nats::Client` already has an inherent `request`, and an inherent
//! method wins over a trait one at every call site.
//!
//! # Wire contract
//!
//! The request announces its encoding in `content-type` — FlatBuffers unless
//! [`NatsRequest::encoding`] says otherwise — and a responder answers in the
//! same encoding, so the reply is decoded as the encoding that was asked for
//! rather than as whatever the reply announces. A `Response::raw` reply,
//! which carries its own content type and arbitrary bytes, is therefore a
//! [`NatsRequestError::Decode`] here.
//!
//! Every request carries an `x-request-id`, generated unless the caller
//! supplies one, and the responder echoes it back.
//!
//! A reply carrying `x-error-code` is a rejection: its code, message, and
//! numeric status are read back into a [`FlatbedRouteError`], so a failure a
//! handler chose reaches the caller as the error the handler returned rather
//! than as a timeout. Structured error details ride in the error reply's
//! payload and are not decoded — the error's status, code, and message come
//! from the headers.
//!
//! Nothing subscribed to the subject is [`NatsRequestError::NoResponders`],
//! distinct from [`NatsRequestError::Timeout`], which means a responder took
//! the request and never answered.

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::time::Duration;

use crate::nats_route::{
    from_nats_headers, header_value, set_header, to_nats_headers, NatsEncoding,
    CONTENT_TYPE_HEADER, ERROR_CODE_HEADER, ERROR_MESSAGE_HEADER, ERROR_STATUS_HEADER,
    REQUEST_ID_HEADER,
};
use crate::{FlatbedRouteError, FromFlatBuffer, HeaderMap, StatusCode, ToFlatBuffer};

/// How long a request waits for its reply when the caller sets no timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything that can go wrong between asking on a subject and holding a
/// decoded reply.
#[derive(Debug)]
pub enum NatsRequestError {
    /// The request body could not be encoded.
    Encode {
        /// Subject the request was bound for.
        subject: String,
        /// The serializer's own message.
        message: String,
    },
    /// Nothing is subscribed to the subject.
    NoResponders {
        /// Subject nobody answers.
        subject: String,
    },
    /// A responder took the request and did not answer in time.
    Timeout {
        /// Subject the request went to.
        subject: String,
        /// How long the request waited.
        timeout: Duration,
    },
    /// The request could not be published, or the connection failed.
    Transport {
        /// Subject the request was bound for.
        subject: String,
        /// The client's own message.
        message: String,
    },
    /// The responder answered with an error reply.
    Reply {
        /// Subject that answered.
        subject: String,
        /// The rejection, rebuilt from the reply's error headers. Its
        /// `headers` are the reply's, so anything else the handler set on
        /// its error reaches the caller too.
        error: FlatbedRouteError<()>,
    },
    /// A reply arrived but does not decode as the response type.
    Decode {
        /// Subject that answered.
        subject: String,
        /// The decoder's own message.
        message: String,
    },
}

impl std::fmt::Display for NatsRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode { subject, message } => {
                write!(f, "request to '{subject}' could not be encoded: {message}")
            }
            Self::NoResponders { subject } => {
                write!(f, "no responder is subscribed to '{subject}'")
            }
            Self::Timeout { subject, timeout } => {
                write!(f, "no reply on '{subject}' within {timeout:?}")
            }
            Self::Transport { subject, message } => {
                write!(f, "request to '{subject}' failed: {message}")
            }
            Self::Reply { subject, error } => {
                write!(f, "'{subject}' rejected the request: {error}")
            }
            Self::Decode { subject, message } => {
                write!(f, "reply on '{subject}' could not be decoded: {message}")
            }
        }
    }
}

impl std::error::Error for NatsRequestError {}

/// Lets a handler propagate a failed subject request with `?`.
///
/// A rejection keeps the status, code, and message the responder chose; every
/// other failure is the subject call itself failing, which is a bad gateway
/// (or a gateway timeout) from the caller's own caller's point of view.
impl From<NatsRequestError> for FlatbedRouteError<()> {
    fn from(err: NatsRequestError) -> Self {
        let message = err.to_string();
        match err {
            NatsRequestError::Reply { error, .. } => error,
            NatsRequestError::Encode { .. } => {
                FlatbedRouteError::internal(message).code("NATS_ENCODE_ERROR")
            }
            NatsRequestError::NoResponders { .. } => {
                FlatbedRouteError::with_status(StatusCode::BAD_GATEWAY, message)
                    .code("NATS_NO_RESPONDERS")
            }
            NatsRequestError::Timeout { .. } => {
                FlatbedRouteError::with_status(StatusCode::GATEWAY_TIMEOUT, message)
                    .code("NATS_TIMEOUT")
            }
            NatsRequestError::Transport { .. } => {
                FlatbedRouteError::with_status(StatusCode::BAD_GATEWAY, message)
                    .code("NATS_TRANSPORT_ERROR")
            }
            NatsRequestError::Decode { .. } => {
                FlatbedRouteError::with_status(StatusCode::BAD_GATEWAY, message)
                    .code("NATS_DECODE_ERROR")
            }
        }
    }
}

/// A typed request waiting to be sent, built by
/// [`NatsRequestExt::typed_request`] and sent by awaiting it.
///
/// `Res` is inferred from what the awaited value is bound to.
#[must_use = "a typed request is only sent when it is awaited"]
pub struct NatsRequest<'a, Req, Res> {
    client: &'a async_nats::Client,
    subject: String,
    body: &'a Req,
    encoding: NatsEncoding,
    timeout: Duration,
    headers: HeaderMap,
    response: PhantomData<fn() -> Res>,
}

impl<Req, Res> NatsRequest<'_, Req, Res> {
    /// Encode the request and decode the reply as `encoding` instead of
    /// FlatBuffers.
    pub fn encoding(mut self, encoding: NatsEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// Wait `timeout` for the reply instead of [`DEFAULT_TIMEOUT`].
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add a header to the request.
    ///
    /// `content-type` is not settable this way: the encoding decides it, and
    /// a request announcing an encoding it did not use would be answered in a
    /// form the reply decode cannot read.
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        set_header(&mut self.headers, name.as_ref(), value.as_ref());
        self
    }
}

impl<Req, Res> IntoFuture for NatsRequest<'_, Req, Res>
where
    Req: ToFlatBuffer,
    Res: FromFlatBuffer + serde::de::DeserializeOwned + Send + 'static,
{
    type Output = Result<Res, NatsRequestError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        let Self {
            client,
            subject,
            body,
            encoding,
            timeout,
            mut headers,
            ..
        } = self;

        let payload = encode(encoding, body).map_err(|message| NatsRequestError::Encode {
            subject: subject.clone(),
            message,
        });

        if header_value(&headers, REQUEST_ID_HEADER).is_none() {
            set_header(
                &mut headers,
                REQUEST_ID_HEADER,
                &uuid::Uuid::new_v4().to_string(),
            );
        }
        set_header(&mut headers, CONTENT_TYPE_HEADER, encoding.content_type());

        let client = client.clone();
        Box::pin(async move {
            let request = async_nats::Request::new()
                .payload(payload?.into())
                .headers(to_nats_headers(&headers))
                .timeout(Some(timeout));

            let reply = client
                .send_request(subject.clone(), request)
                .await
                .map_err(|e| transport_error(&subject, timeout, &e))?;

            let reply_headers = from_nats_headers(reply.headers.as_ref());
            if header_value(&reply_headers, ERROR_CODE_HEADER).is_some() {
                return Err(NatsRequestError::Reply {
                    subject,
                    error: rejection(reply_headers),
                });
            }

            decode(encoding, &reply.payload)
                .map_err(|message| NatsRequestError::Decode { subject, message })
        })
    }
}

/// Sends a typed request on a core-NATS subject.
///
/// Implemented for `async_nats::Client`, so any context handing out a client —
/// [`HasNatsClient`](crate::HasNatsClient) among them — can ask on a subject.
pub trait NatsRequestExt {
    /// Build a request carrying `body` on `subject`.
    ///
    /// Nothing is sent until the returned [`NatsRequest`] is awaited.
    fn typed_request<'a, Req, Res>(
        &'a self,
        subject: impl Into<String>,
        body: &'a Req,
    ) -> NatsRequest<'a, Req, Res>;
}

impl NatsRequestExt for async_nats::Client {
    fn typed_request<'a, Req, Res>(
        &'a self,
        subject: impl Into<String>,
        body: &'a Req,
    ) -> NatsRequest<'a, Req, Res> {
        NatsRequest {
            client: self,
            subject: subject.into(),
            body,
            encoding: NatsEncoding::FlatBuffer,
            timeout: DEFAULT_TIMEOUT,
            headers: HeaderMap::new(),
            response: PhantomData,
        }
    }
}

fn encode<Req: ToFlatBuffer>(encoding: NatsEncoding, body: &Req) -> Result<Vec<u8>, String> {
    match encoding {
        NatsEncoding::Json => serde_json::to_vec(body).map_err(|e| e.to_string()),
        NatsEncoding::FlatBuffer => Ok(body.to_flatbuffer()),
    }
}

fn decode<Res: FromFlatBuffer + serde::de::DeserializeOwned>(
    encoding: NatsEncoding,
    payload: &[u8],
) -> Result<Res, String> {
    match encoding {
        NatsEncoding::Json => serde_json::from_slice(payload).map_err(|e| e.to_string()),
        NatsEncoding::FlatBuffer => Res::from_flatbuffer(payload).map_err(|e| e.to_string()),
    }
}

fn transport_error(
    subject: &str,
    timeout: Duration,
    err: &async_nats::RequestError,
) -> NatsRequestError {
    match err.kind() {
        async_nats::RequestErrorKind::TimedOut => NatsRequestError::Timeout {
            subject: subject.to_string(),
            timeout,
        },
        async_nats::RequestErrorKind::NoResponders => NatsRequestError::NoResponders {
            subject: subject.to_string(),
        },
        async_nats::RequestErrorKind::Other => NatsRequestError::Transport {
            subject: subject.to_string(),
            message: err.to_string(),
        },
    }
}

/// Rebuild the error a responder returned from the headers it discriminates
/// replies with.
fn rejection(headers: HeaderMap) -> FlatbedRouteError<()> {
    let status = header_value(&headers, ERROR_STATUS_HEADER)
        .and_then(|status| StatusCode::from_bytes(status.as_bytes()).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    FlatbedRouteError {
        status,
        code: header_value(&headers, ERROR_CODE_HEADER)
            .unwrap_or("ERROR")
            .to_string(),
        message: header_value(&headers, ERROR_MESSAGE_HEADER)
            .unwrap_or_default()
            .to_string(),
        headers,
        details: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            set_header(&mut headers, name, value);
        }
        headers
    }

    #[test]
    fn a_rejection_carries_the_status_code_and_message_the_responder_chose() {
        let error = rejection(headers(&[
            (ERROR_CODE_HEADER, "NOT_FOUND"),
            (ERROR_MESSAGE_HEADER, "no such satellite"),
            (ERROR_STATUS_HEADER, "404"),
            ("x-trace", "t-9"),
        ]));

        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "NOT_FOUND");
        assert_eq!(error.message, "no such satellite");
        assert_eq!(
            header_value(&error.headers, "x-trace"),
            Some("t-9"),
            "the rest of the reply's headers reach the caller"
        );
    }

    /// The status header is the responder's, so a value that is not an HTTP
    /// status must not decide the caller's status by accident.
    #[test]
    fn a_rejection_without_a_readable_status_falls_back_to_internal_error() {
        let error = rejection(headers(&[(ERROR_CODE_HEADER, "BOOM")]));

        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.message, "");

        let unreadable = rejection(headers(&[
            (ERROR_CODE_HEADER, "BOOM"),
            (ERROR_STATUS_HEADER, "not-a-status"),
        ]));
        assert_eq!(unreadable.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A handler propagating a subject call with `?` must not report the
    /// responder's rejection as the caller's own fault, and must not report a
    /// dead subject as a success-shaped failure.
    #[test]
    fn propagating_a_failure_maps_each_kind_onto_a_distinct_status() {
        let rejected: FlatbedRouteError<()> = NatsRequestError::Reply {
            subject: "a.b".to_string(),
            error: FlatbedRouteError::not_found("gone").code("NOT_FOUND"),
        }
        .into();
        assert_eq!(rejected.status, StatusCode::NOT_FOUND);
        assert_eq!(rejected.code, "NOT_FOUND");

        let timed_out: FlatbedRouteError<()> = NatsRequestError::Timeout {
            subject: "a.b".to_string(),
            timeout: Duration::from_secs(2),
        }
        .into();
        assert_eq!(timed_out.status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(timed_out.code, "NATS_TIMEOUT");
        assert_eq!(timed_out.message, "no reply on 'a.b' within 2s");

        let unreachable: FlatbedRouteError<()> = NatsRequestError::NoResponders {
            subject: "a.b".to_string(),
        }
        .into();
        assert_eq!(unreachable.status, StatusCode::BAD_GATEWAY);
        assert_eq!(unreachable.code, "NATS_NO_RESPONDERS");

        let undecodable: FlatbedRouteError<()> = NatsRequestError::Decode {
            subject: "a.b".to_string(),
            message: "bad buffer".to_string(),
        }
        .into();
        assert_eq!(undecodable.status, StatusCode::BAD_GATEWAY);
        assert_eq!(undecodable.code, "NATS_DECODE_ERROR");
    }
}
