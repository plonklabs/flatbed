//! NATS JetStream support types for flatbed workers.
//!
//! These types are used by the [`StreamWorker`] trait to provide a
//! clean handler interface for JetStream consumers.

use std::sync::Arc;

use tracing::{error, info, warn};

use crate::FlatbedWorkerError;

/// Result type for NATS worker handlers.
///
/// Determines how the framework handles message acknowledgment:
/// - `Ack` — message processed, ACK sent
/// - `Skip` — message skipped, ACK sent (prevents redelivery), reason logged
/// - `Retry` — processing failed, no ACK (message redelivered), reason logged
///
/// # Example
///
/// ```rust,ignore
/// fn handle(&self, ctx: Arc<AppContext>, msg: DoThing) -> BoxFuture<NatsResult> {
///     Box::pin(async move {
///         match do_work(&ctx, &msg).await {
///             Ok(_) => NatsResult::Ack,
///             Err(e) if e.is_permanent() => NatsResult::Skip(format!("Permanent error: {}", e)),
///             Err(e) => NatsResult::Retry(format!("Transient error: {}", e)),
///         }
///     })
/// }
/// ```
pub enum NatsResult {
    /// Message processed successfully. ACK.
    Ack,
    /// Skip this message. ACK to prevent redelivery, log reason.
    Skip(String),
    /// Processing failed, retry later. Don't ACK, message redelivered.
    Retry(String),
}

/// Trait for application contexts that provide a JetStream context.
///
/// Implement this trait on your application context type to allow
/// [`StreamWorker`] and [`crate::k8s::KubeReconciler`] implementors to
/// access JetStream.
///
/// # Example
///
/// ```rust,ignore
/// struct AppContext {
///     jetstream: async_nats::jetstream::Context,
/// }
///
/// impl HasJetStream for AppContext {
///     fn jetstream(&self) -> &async_nats::jetstream::Context {
///         &self.jetstream
///     }
/// }
/// ```
pub trait HasJetStream {
    /// Returns a reference to the JetStream context.
    fn jetstream(&self) -> &async_nats::jetstream::Context;

    /// Number of replicas to create JetStream streams with.
    ///
    /// Defaults to 1 (single-server JetStream). Contexts running
    /// against a clustered NATS deployment (e.g. an operator with
    /// multiple sidecars connected as peers) override this to
    /// match their peer count, so streams created via the
    /// reconciler-controller loop are replicated for survival
    /// across a single peer loss.
    fn stream_replicas(&self) -> usize {
        1
    }
}

// ============================================================================
// StreamWorker Trait
// ============================================================================

/// Trait-based NATS JetStream consumer worker.
///
/// Implement this trait to define a JetStream queue consumer. The
/// runtime executor [`run_stream_worker`] handles HA mode (follower-only
/// consumption), consumer setup, and message dispatch.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::nats::{StreamWorker, NatsResult};
/// use std::sync::Arc;
///
/// struct MyWorker;
///
/// impl StreamWorker for MyWorker {
///     type Message = MyMessage;
///     type Context = AppContext;
///
///     const NAME: &'static str = "my-worker";
///     const STREAM: &'static str = "TASKS";
///     const SUBJECT: &'static str = "tasks.do_thing";
///
///     fn handle(
///         &self,
///         ctx: Arc<Self::Context>,
///         msg: Self::Message,
///     ) -> flatbed::BoxFuture<NatsResult> {
///         Box::pin(async move { NatsResult::Ack })
///     }
///
///     type ParseError = String;
///
///     fn parse_message(bytes: &[u8]) -> Result<Self::Message, Self::ParseError> {
///         MyMessage::from_flatbuffer(bytes).map_err(|e| e.to_string())
///     }
/// }
///
/// flatbed::register_stream_worker!(MyWorker, AppContext);
/// ```
///
/// `ParseError` is an associated type so workers with multiple
/// failure modes (e.g. one variant per missing required field) can
/// return a `thiserror` enum the runtime logs via `Display` rather
/// than collapsing every distinction into a single `String`.
/// Workers with one trivial failure mode (the common
/// `from_flatbuffer` decode) can keep `type ParseError = String;`.
pub trait StreamWorker: Send + Sync + 'static {
    /// The message type consumed from the stream.
    type Message: Send + 'static;

    /// The application context type providing JetStream and leader election.
    #[cfg(all(feature = "nats", feature = "k8s"))]
    type Context: HasJetStream + crate::k8s::HasLeaderElection + Send + Sync + 'static;

    /// The application context type providing JetStream.
    #[cfg(not(feature = "k8s"))]
    type Context: HasJetStream + Send + Sync + 'static;

    /// Error type returned by [`Self::parse_message`]. The runtime
    /// logs failures via `Display` and ACKs the message to skip; no
    /// trait bound forces a particular shape, so workers can pick
    /// `String` for trivial single-mode parses or a typed enum when
    /// failure modes need to be distinguishable in triage.
    type ParseError: std::fmt::Display + Send + 'static;

    /// Worker name used for logging, durable consumer name, and identification.
    const NAME: &'static str;

    /// Optional description of what this worker does.
    const DESCRIPTION: Option<&'static str> = None;

    /// JetStream stream name to consume from.
    const STREAM: &'static str;

    /// JetStream subject filter for the consumer.
    const SUBJECT: &'static str;

    /// Handle a single deserialized message.
    fn handle(&self, ctx: Arc<Self::Context>, msg: Self::Message) -> crate::BoxFuture<NatsResult>;

    /// Deserialize raw message bytes into the message type.
    fn parse_message(bytes: &[u8]) -> Result<Self::Message, Self::ParseError>;
}

// ============================================================================
// StreamWorker Runtime Executor
// ============================================================================

/// Internal consume loop that creates a pull consumer and dispatches messages.
async fn consume_loop<W, C>(worker: &W, ctx: Arc<C>) -> Result<(), FlatbedWorkerError>
where
    W: StreamWorker<Context = C>,
    C: HasJetStream + Send + Sync + 'static,
{
    let jetstream = HasJetStream::jetstream(&*ctx);

    // Wait for the stream to exist before creating a consumer.
    //
    // Streams are created by reconcilers (see
    // `crate::k8s::reconcile_loop`) once the pod acquires leadership.
    // At process startup the `is_leader` watch defaults to `false`,
    // so workers reach this point BEFORE any reconciler has acquired
    // leadership and called `get_or_create_stream`. Without the
    // wait, every worker fails its first `create_consumer_on_stream`
    // call with a 404 (`stream not found`) and the operator exits
    // cleanly — taking restart attempts to converge purely on the
    // ordering of the K8s restart loop.
    //
    // We can't call `get_or_create_stream` here because workers
    // don't carry the full stream config (subjects, retention,
    // storage class) — only the reconciler knows that. Instead we
    // poll for stream existence with bounded backoff and let the
    // reconciler be the sole stream creator.
    let backoff_ms = [100u64, 250, 500, 1_000, 2_000, 5_000];
    let mut attempt = 0;
    loop {
        match jetstream.get_stream(W::STREAM).await {
            Ok(_) => break,
            Err(e) => {
                let delay = backoff_ms[attempt.min(backoff_ms.len() - 1)];
                tracing::debug!(
                    worker = W::NAME,
                    stream = W::STREAM,
                    attempt,
                    delay_ms = delay,
                    error = %e,
                    "stream not yet available, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                attempt += 1;
            }
        }
    }

    let consumer = jetstream
        .create_consumer_on_stream(
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(W::NAME.to_string()),
                filter_subject: W::SUBJECT.to_string(),
                ..Default::default()
            },
            W::STREAM,
        )
        .await
        .map_err(|e| {
            FlatbedWorkerError::new(
                "consumer_init",
                format!("[{}] JetStream consumer failed: {}", W::NAME, e),
            )
        })?;

    info!(worker = W::NAME, subject = W::SUBJECT, "subscribed");

    let mut messages = consumer.messages().await.map_err(|e| {
        FlatbedWorkerError::new(
            "messages_init",
            format!("[{}] Failed to get messages: {}", W::NAME, e),
        )
    })?;

    use futures::StreamExt;

    while let Some(msg_result) = messages.next().await {
        let nats_msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                error!(worker = W::NAME, error = %e, "message receive error");
                continue;
            }
        };

        let payload = nats_msg.payload.as_ref();
        let decoded = match W::parse_message(payload) {
            Ok(m) => m,
            Err(e) => {
                warn!(worker = W::NAME, error = %e, "deserialization failed, ACKing to skip");
                if let Err(ack_err) = nats_msg.ack().await {
                    error!(worker = W::NAME, error = %ack_err, "failed to ACK message");
                }
                continue;
            }
        };

        match worker.handle(Arc::clone(&ctx), decoded).await {
            NatsResult::Ack => {
                if let Err(ack_err) = nats_msg.ack().await {
                    error!(worker = W::NAME, error = %ack_err, "failed to ACK message");
                }
            }
            NatsResult::Skip(reason) => {
                info!(worker = W::NAME, reason = %reason, "skipping message");
                if let Err(ack_err) = nats_msg.ack().await {
                    error!(worker = W::NAME, error = %ack_err, "failed to ACK message");
                }
            }
            NatsResult::Retry(reason) => {
                warn!(worker = W::NAME, reason = %reason, "retrying message");
                // Don't ack — message will be redelivered
            }
        }
    }

    Ok(())
}

/// Run a [`StreamWorker`] with optional HA-aware follower-only consumption.
///
/// This is the runtime executor that powers `register_stream_worker!`,
/// implementing the following lifecycle:
///
/// - **Without k8s feature or HA disabled**: consume directly (standalone mode)
/// - **With k8s + HA enabled**: only consume on follower pods, stopping when
///   the pod becomes leader (via `tokio::select!`)
#[cfg(feature = "k8s")]
pub async fn run_stream_worker<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    W: StreamWorker<Context = C> + Default,
    C: HasJetStream + crate::k8s::HasLeaderElection + Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("stream_worker '{}' context type mismatch", W::NAME));

    let worker = W::default();

    if !crate::k8s::HasLeaderElection::ha_enabled(&*ctx) {
        info!(worker = W::NAME, "standalone mode, consuming directly");
        return consume_loop(&worker, ctx).await;
    }

    let mut is_leader_rx = crate::k8s::HasLeaderElection::is_leader_rx(&*ctx);

    loop {
        if !crate::k8s::wait_for_follower(&mut is_leader_rx, W::NAME).await {
            return Ok(());
        }

        info!(worker = W::NAME, "follower mode, starting consumer");

        let mut rx2 = crate::k8s::HasLeaderElection::is_leader_rx(&*ctx);

        tokio::select! {
            result = consume_loop(&worker, Arc::clone(&ctx)) => {
                result?;
            }
            _ = crate::k8s::wait_for_follower_loss(&mut rx2, W::NAME) => {
                info!(worker = W::NAME, "became leader, stopping consumer");
            }
        }
    }
}

/// Run a [`StreamWorker`] in standalone mode (no leader election).
///
/// This variant is used when the k8s feature is not enabled.
#[cfg(not(feature = "k8s"))]
pub async fn run_stream_worker<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    W: StreamWorker<Context = C> + Default,
    C: HasJetStream + Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("stream_worker '{}' context type mismatch", W::NAME));

    let worker = W::default();
    consume_loop(&worker, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- NatsResult ---

    #[test]
    fn nats_result_ack_variant() {
        // Verify the Ack variant can be constructed
        let result = NatsResult::Ack;
        assert!(matches!(result, NatsResult::Ack));
    }

    #[test]
    fn nats_result_skip_variant() {
        let result = NatsResult::Skip("duplicate message".to_string());
        match result {
            NatsResult::Skip(reason) => assert_eq!(reason, "duplicate message"),
            _ => panic!("Expected Skip variant"),
        }
    }

    #[test]
    fn nats_result_retry_variant() {
        let result = NatsResult::Retry("transient error".to_string());
        match result {
            NatsResult::Retry(reason) => assert_eq!(reason, "transient error"),
            _ => panic!("Expected Retry variant"),
        }
    }

    // --- StreamWorker parse_message ---

    /// A simple test message type for verifying parse_message behavior.
    #[derive(Debug, PartialEq)]
    struct TestMessage {
        value: u32,
    }

    /// Minimal StreamWorker implementation for testing parse_message.
    struct TestStreamWorker;

    impl Default for TestStreamWorker {
        fn default() -> Self {
            TestStreamWorker
        }
    }

    /// Minimal context stub for tests -- only needs HasJetStream + HasLeaderElection.
    /// Since we cannot construct a real JetStream context without a NATS server,
    /// these tests only exercise parse_message which is a static method.
    impl StreamWorker for TestStreamWorker {
        type Message = TestMessage;

        #[cfg(all(feature = "nats", feature = "k8s"))]
        type Context = TestStreamWorkerCtx;

        #[cfg(not(feature = "k8s"))]
        type Context = TestStreamWorkerCtx;

        type ParseError = String;

        const NAME: &'static str = "test-stream-worker";
        const STREAM: &'static str = "TEST_STREAM";
        const SUBJECT: &'static str = "test.subject";

        fn handle(
            &self,
            _ctx: Arc<Self::Context>,
            _msg: Self::Message,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NatsResult> + Send>> {
            Box::pin(async { NatsResult::Ack })
        }

        fn parse_message(bytes: &[u8]) -> Result<Self::Message, Self::ParseError> {
            // Simple parse: expect exactly 4 bytes as a little-endian u32
            if bytes.len() != 4 {
                return Err(format!("expected 4 bytes, got {}", bytes.len()));
            }
            let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(TestMessage { value })
        }
    }

    /// Stub context for the test StreamWorker. Not actually used by parse_message tests.
    struct TestStreamWorkerCtx;

    impl HasJetStream for TestStreamWorkerCtx {
        fn jetstream(&self) -> &async_nats::jetstream::Context {
            panic!("StubJetStream: not expected to be called in parse_message tests");
        }
    }

    #[cfg(feature = "k8s")]
    impl crate::k8s::HasLeaderElection for TestStreamWorkerCtx {
        fn is_leader_rx(&self) -> tokio::sync::watch::Receiver<bool> {
            panic!("StubLeaderElection: not expected to be called in parse_message tests");
        }
    }

    #[test]
    fn parse_message_valid_bytes() {
        let bytes = 42u32.to_le_bytes();
        let result = TestStreamWorker::parse_message(&bytes);
        assert_eq!(result, Ok(TestMessage { value: 42 }));
    }

    #[test]
    fn parse_message_invalid_bytes_returns_error() {
        // Too few bytes
        let result = TestStreamWorker::parse_message(&[1, 2]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 4 bytes"));
    }

    #[test]
    fn parse_message_empty_bytes_returns_error() {
        let result = TestStreamWorker::parse_message(&[]);
        assert!(result.is_err());
    }

    // --- Typed (non-String) ParseError ---

    /// A custom enum standing in for the kind of typed error a
    /// downstream worker might return through the widened trait.
    /// Pinning a non-`String` `ParseError` at the flatbed level
    /// catches a regression that re-narrows the bound to `String`
    /// — that would silently break every downstream consumer
    /// depending on the wider bound.
    /// `Display` is hand-rolled to avoid a `thiserror` dev-dep on
    /// flatbed; the runtime only needs `Display` and that's what's
    /// pinned here.
    #[derive(Debug, PartialEq)]
    enum CustomParseError {
        TooShort { got: usize, expected: usize },
        Empty,
    }

    impl std::fmt::Display for CustomParseError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::TooShort { got, expected } => {
                    write!(f, "payload too short: got {got}, expected {expected}")
                }
                Self::Empty => write!(f, "payload empty"),
            }
        }
    }

    struct TypedErrorStreamWorker;

    impl StreamWorker for TypedErrorStreamWorker {
        type Message = TestMessage;

        #[cfg(all(feature = "nats", feature = "k8s"))]
        type Context = TestStreamWorkerCtx;

        #[cfg(not(feature = "k8s"))]
        type Context = TestStreamWorkerCtx;

        type ParseError = CustomParseError;

        const NAME: &'static str = "typed-error-stream-worker";
        const STREAM: &'static str = "TEST_STREAM";
        const SUBJECT: &'static str = "test.typed";

        fn handle(
            &self,
            _ctx: Arc<Self::Context>,
            _msg: Self::Message,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NatsResult> + Send>> {
            Box::pin(async { NatsResult::Ack })
        }

        fn parse_message(bytes: &[u8]) -> Result<Self::Message, Self::ParseError> {
            if bytes.is_empty() {
                return Err(CustomParseError::Empty);
            }
            if bytes.len() < 4 {
                return Err(CustomParseError::TooShort {
                    got: bytes.len(),
                    expected: 4,
                });
            }
            let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Ok(TestMessage { value })
        }
    }

    /// A typed `ParseError` round-trips through `parse_message` —
    /// the caller gets the structured variant, not a stringified
    /// form. Pins that the widened bound accepts non-`String`
    /// error types so a re-narrowing regression surfaces here
    /// first.
    #[test]
    fn typed_parse_error_round_trips_structured_variant() {
        let err = TypedErrorStreamWorker::parse_message(&[1, 2]).unwrap_err();
        assert_eq!(
            err,
            CustomParseError::TooShort {
                got: 2,
                expected: 4
            }
        );

        let err_empty = TypedErrorStreamWorker::parse_message(&[]).unwrap_err();
        assert_eq!(err_empty, CustomParseError::Empty);
    }

    /// `Display` is what the runtime's `warn!(error = %e, …)` call
    /// site renders. Pin the formatted output so a refactor that
    /// changes the `Display` impl template surfaces here.
    #[test]
    fn typed_parse_error_display_matches_expected_template() {
        let err = CustomParseError::TooShort {
            got: 2,
            expected: 4,
        };
        assert_eq!(err.to_string(), "payload too short: got 2, expected 4");
        assert_eq!(CustomParseError::Empty.to_string(), "payload empty");
    }

    // --- StreamWorker trait defaults ---

    #[test]
    fn stream_worker_description_default_is_none() {
        assert!(
            <TestStreamWorker as StreamWorker>::DESCRIPTION.is_none(),
            "Default DESCRIPTION should be None"
        );
    }

    #[test]
    fn stream_worker_constants_match() {
        assert_eq!(
            <TestStreamWorker as StreamWorker>::NAME,
            "test-stream-worker"
        );
        assert_eq!(<TestStreamWorker as StreamWorker>::STREAM, "TEST_STREAM");
        assert_eq!(<TestStreamWorker as StreamWorker>::SUBJECT, "test.subject");
    }

    #[test]
    fn stream_replicas_default_is_one() {
        struct DefaultCtx;
        impl HasJetStream for DefaultCtx {
            fn jetstream(&self) -> &async_nats::jetstream::Context {
                panic!("not called in this test")
            }
        }
        assert_eq!(DefaultCtx.stream_replicas(), 1);
    }

    #[test]
    fn stream_replicas_override_returns_override_value() {
        struct OverrideCtx;
        impl HasJetStream for OverrideCtx {
            fn jetstream(&self) -> &async_nats::jetstream::Context {
                panic!("not called in this test")
            }
            fn stream_replicas(&self) -> usize {
                3
            }
        }
        assert_eq!(OverrideCtx.stream_replicas(), 3);
    }
}
