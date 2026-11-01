//! In-process message worker: a bounded, keyed queue handled one message at
//! a time.
//!
//! Producers inside the process enqueue through a [`MessageSender`] held on
//! the application context; one [`MessageWorker`] takes the messages strictly
//! in order, taking the next only after [`handle`](MessageWorker::handle)
//! returns. Nothing is persisted: the queue is lost on restart, so producers
//! are expected to be level-triggered and resend what they still need.
//!
//! ## Queue semantics
//!
//! - **Coalescing.** Every message has a [`key`](MessageWorker::key). Sending
//!   a key that is already queued adds nothing: the queued entry keeps both
//!   its place in line and its original payload, and the new payload is
//!   discarded. The message being handled counts as *not* queued, so a resend
//!   during handling queues the key again, to run after the current attempt.
//! - **Position.** [`MessageSender::position`] is `Some(0)` for the message
//!   being handled, `Some(1)` for the next one, and `None` for a key that is
//!   neither.
//! - **Capacity.** At most [`CAPACITY`](MessageWorker::CAPACITY) messages are
//!   held, counting the one being handled. Sending a new key into a full
//!   queue returns [`QueueFull`]; coalescing into a queued key always
//!   succeeds.
//! - **Outcome.** [`MessageResult::Retry`] puts the message back at the end
//!   of the line, unless the same key was resent meanwhile, in which case the
//!   queued resend stands in for it. After a retry the worker waits
//!   [`RETRY_DELAY`](MessageWorker::RETRY_DELAY) before taking the next
//!   message, so messages that keep failing never spin. An `Err` from `handle`
//!   is logged and the message dropped; it does not stop the worker or touch
//!   health.
//!
//! ## Leadership
//!
//! With the `k8s` feature the context provides leader election (see
//! [`MessageWorker::Context`]), and messages are handled only while this
//! process leads. Each time it becomes leader the worker waits
//! [`SETTLE`](MessageWorker::SETTLE) before the first message. When it loses
//! leadership the message being handled is cancelled and the whole queue is
//! discarded. Sends are accepted whether or not the process leads; what a
//! follower queues waits for the next leadership term, and is discarded with
//! the rest if leadership is lost first. With leader election disabled
//! (`ha_enabled() == false`), or without the `k8s` feature, the worker
//! handles messages from startup, after one `SETTLE`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::{BoxFuture, FlatbedWorkerError};

/// Outcome of handling one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageResult {
    /// The message is finished.
    Done,
    /// Put the message back at the end of the line; the reason is logged.
    Retry(String),
}

/// In-process message worker handling a keyed queue one message at a time.
///
/// See the [module docs](self) for the queue and leadership semantics.
///
/// # Requirements
///
/// Implementors must also implement `Default`: the runtime executor builds
/// the worker with `W::default()` each time it starts.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::message::{MessageResult, MessageSender, MessageWorker};
/// use std::sync::Arc;
///
/// #[derive(Clone)]
/// struct RunMigrator { namespace: String, migrator: String }
///
/// #[derive(Default)]
/// struct MigratorRunner;
///
/// impl MessageWorker for MigratorRunner {
///     type Message = RunMigrator;
///     type Key = (String, String);
///     type Context = AppContext;
///     type Error = kube::Error;
///
///     const NAME: &'static str = "migrator-runner";
///
///     fn key(msg: &RunMigrator) -> Self::Key {
///         (msg.namespace.clone(), msg.migrator.clone())
///     }
///
///     fn sender(ctx: &AppContext) -> &MessageSender<Self> {
///         &ctx.migrations
///     }
///
///     fn handle(
///         &self,
///         ctx: Arc<AppContext>,
///         msg: RunMigrator,
///     ) -> flatbed::BoxFuture<Result<MessageResult, kube::Error>> {
///         Box::pin(async move {
///             if !run_job(&ctx, &msg).await? {
///                 return Ok(MessageResult::Retry("job not finished".into()));
///             }
///             Ok(MessageResult::Done)
///         })
///     }
/// }
///
/// flatbed::register_message_worker!(MigratorRunner, AppContext);
/// ```
pub trait MessageWorker: Sized + Send + Sync + 'static {
    /// The message type carried by the queue. Cloned once per attempt so a
    /// retry can re-enqueue it.
    type Message: Clone + Send + 'static;

    /// Identity used to coalesce messages; see [`key`](Self::key).
    type Key: Eq + Send + 'static;

    /// The application context type holding the [`MessageSender`] and
    /// providing leader election.
    #[cfg(feature = "k8s")]
    type Context: crate::k8s::HasLeaderElection + Send + Sync + 'static;

    /// The application context type holding the [`MessageSender`].
    #[cfg(not(feature = "k8s"))]
    type Context: Send + Sync + 'static;

    /// Error returned by [`handle`](Self::handle). The runtime logs it via
    /// `Display` and drops the message.
    type Error: fmt::Display + Send + 'static;

    /// Worker name used for logging and identification.
    const NAME: &'static str;

    /// Optional description of what this worker does.
    const DESCRIPTION: Option<&'static str> = None;

    /// Opt-in in-process restart policy. `None` — the default — makes the
    /// worker's first exit terminal, leaving Kubernetes to restart the pod.
    /// Registration puts this in a `static` initializer, so the expression
    /// has to be const-evaluatable; the [`RestartPolicy`](crate::RestartPolicy)
    /// builders are `const fn`. A restart keeps the queue but drops the
    /// message that was being handled.
    const RESTART: ::core::option::Option<crate::RestartPolicy> = ::core::option::Option::None;

    /// Most messages held at once, counting the one being handled.
    const CAPACITY: usize = 1024;

    /// Pause after becoming leader before the first message is taken, giving
    /// producers time to resend. Defaults to zero.
    const SETTLE: Duration = Duration::ZERO;

    /// Pause after a [`MessageResult::Retry`] before the next message is
    /// taken.
    const RETRY_DELAY: Duration = Duration::from_secs(1);

    /// The coalescing key of a message: two messages with equal keys are the
    /// same request.
    fn key(msg: &Self::Message) -> Self::Key;

    /// The sender held on the context. The worker takes its messages from
    /// the same queue, so this must return the same sender every time.
    fn sender(ctx: &Self::Context) -> &MessageSender<Self>;

    /// Handle one message. The next message is taken only after the
    /// returned future completes. The future is dropped mid-flight when
    /// leadership is lost.
    fn handle(
        &self,
        ctx: Arc<Self::Context>,
        msg: Self::Message,
    ) -> BoxFuture<Result<MessageResult, Self::Error>>;

    /// Finish in-progress work during graceful shutdown, within the
    /// configured shutdown budget. Defaults to a no-op.
    fn drain(&self, _ctx: Arc<Self::Context>) -> BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Sending a new key into a queue already holding
/// [`MessageWorker::CAPACITY`] messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull {
    /// The worker's [`MessageWorker::NAME`].
    pub worker: &'static str,
    /// The worker's [`MessageWorker::CAPACITY`].
    pub capacity: usize,
}

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "message worker '{}' queue is full ({} messages)",
            self.worker, self.capacity
        )
    }
}

impl std::error::Error for QueueFull {}

/// Enqueues messages for a [`MessageWorker`].
///
/// Create one with [`MessageSender::new`] while building the application
/// context and return it from [`MessageWorker::sender`]. Clones share one
/// queue and are cheap. Sending never waits for the message to be handled.
pub struct MessageSender<W: MessageWorker> {
    shared: Arc<Shared<W>>,
}

struct Shared<W: MessageWorker> {
    state: Mutex<State<W>>,
    wake: Notify,
}

struct State<W: MessageWorker> {
    queued: VecDeque<(W::Key, W::Message)>,
    in_flight: Option<W::Key>,
}

impl<W: MessageWorker> State<W> {
    fn is_queued(&self, key: &W::Key) -> bool {
        self.queued.iter().any(|(queued, _)| queued == key)
    }
}

impl<W: MessageWorker> MessageSender<W> {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queued: VecDeque::new(),
                    in_flight: None,
                }),
                wake: Notify::new(),
            }),
        }
    }

    /// Enqueue `msg` at the end of the line, or coalesce it into the queued
    /// message with the same key, which keeps its place and payload.
    ///
    /// # Errors
    ///
    /// [`QueueFull`] when the key is not queued and the queue already holds
    /// [`MessageWorker::CAPACITY`] messages.
    pub fn send(&self, msg: W::Message) -> Result<(), QueueFull> {
        let key = W::key(&msg);
        let mut state = self.lock();
        if state.is_queued(&key) {
            return Ok(());
        }
        if state.queued.len() + usize::from(state.in_flight.is_some()) >= W::CAPACITY {
            return Err(QueueFull {
                worker: W::NAME,
                capacity: W::CAPACITY,
            });
        }
        state.queued.push_back((key, msg));
        drop(state);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Where `key` stands: `Some(0)` while it is being handled, `Some(1)`
    /// when it is next, `None` when it is neither handled nor queued. A key
    /// resent during its own handling reports `Some(0)` until that attempt
    /// finishes.
    #[must_use]
    pub fn position(&self, key: &W::Key) -> Option<usize> {
        let state = self.lock();
        if state.in_flight.as_ref() == Some(key) {
            return Some(0);
        }
        state
            .queued
            .iter()
            .position(|(queued, _)| queued == key)
            .map(|index| index + 1)
    }

    fn lock(&self) -> MutexGuard<'_, State<W>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn next(&self) -> W::Message {
        loop {
            let entry = {
                let mut state = self.lock();
                state.queued.pop_front().map(|(key, msg)| {
                    state.in_flight = Some(key);
                    msg
                })
            };
            if let Some(msg) = entry {
                return msg;
            }
            self.shared.wake.notified().await;
        }
    }

    fn finish(&self, retry: Option<W::Message>) {
        let mut state = self.lock();
        let Some(key) = state.in_flight.take() else {
            return;
        };
        let Some(msg) = retry else {
            return;
        };
        if !state.is_queued(&key) {
            state.queued.push_back((key, msg));
        }
    }

    fn release_in_flight(&self) {
        self.lock().in_flight = None;
    }

    #[cfg(feature = "k8s")]
    fn clear(&self) -> usize {
        let mut state = self.lock();
        let dropped = state.queued.len() + usize::from(state.in_flight.is_some());
        state.queued.clear();
        state.in_flight = None;
        dropped
    }
}

impl<W: MessageWorker> Default for MessageSender<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: MessageWorker> Clone for MessageSender<W> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<W: MessageWorker> fmt::Debug for MessageSender<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageSender")
            .field("worker", &W::NAME)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Runtime Executor
// ============================================================================

/// Run a [`MessageWorker`]: the executor behind `register_message_worker!`.
pub async fn run_message_worker<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    W: MessageWorker<Context = C> + Default,
    C: Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("message_worker '{}' context type mismatch", W::NAME));
    serve(W::default(), ctx).await
}

#[cfg(feature = "k8s")]
async fn serve<W: MessageWorker>(
    worker: W,
    ctx: Arc<W::Context>,
) -> Result<(), FlatbedWorkerError> {
    use crate::k8s::{wait_for_leadership, wait_for_leadership_loss, HasLeaderElection};

    let sender = W::sender(&ctx).clone();
    sender.release_in_flight();

    if !ctx.ha_enabled() {
        info!(worker = W::NAME, "standalone mode, handling messages");
        handle_term(&worker, &ctx, &sender).await;
        return Ok(());
    }

    let mut is_leader_rx = ctx.is_leader_rx();
    loop {
        if !wait_for_leadership(&mut is_leader_rx, W::NAME).await {
            return Ok(());
        }
        info!(worker = W::NAME, "became leader, handling messages");

        let mut loss_rx = ctx.is_leader_rx();
        tokio::select! {
            () = handle_term(&worker, &ctx, &sender) => {}
            () = wait_for_leadership_loss(&mut loss_rx, W::NAME) => {}
        }
        let dropped = sender.clear();
        info!(
            worker = W::NAME,
            dropped, "lost leadership, dropped queued messages"
        );
    }
}

#[cfg(not(feature = "k8s"))]
async fn serve<W: MessageWorker>(
    worker: W,
    ctx: Arc<W::Context>,
) -> Result<(), FlatbedWorkerError> {
    let sender = W::sender(&ctx).clone();
    sender.release_in_flight();
    info!(worker = W::NAME, "handling messages");
    handle_term(&worker, &ctx, &sender).await;
    Ok(())
}

/// Settle, then handle messages until cancelled. Never returns.
async fn handle_term<W: MessageWorker>(
    worker: &W,
    ctx: &Arc<W::Context>,
    sender: &MessageSender<W>,
) {
    tokio::time::sleep(W::SETTLE).await;
    loop {
        let msg = sender.next().await;
        let retry = match worker.handle(Arc::clone(ctx), msg.clone()).await {
            Ok(MessageResult::Done) => {
                debug!(worker = W::NAME, "message done");
                None
            }
            Ok(MessageResult::Retry(reason)) => {
                warn!(worker = W::NAME, reason = %reason, "retrying message");
                Some(msg)
            }
            Err(e) => {
                error!(worker = W::NAME, error = %e, "message failed, dropping it");
                None
            }
        };
        let retried = retry.is_some();
        sender.finish(retry);
        if retried {
            tokio::time::sleep(W::RETRY_DELAY).await;
        }
    }
}

/// Drain a [`MessageWorker`] by downcasting the context and delegating to
/// `MessageWorker::drain`.
pub fn run_message_worker_drain<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> BoxFuture<Result<(), FlatbedWorkerError>>
where
    W: MessageWorker<Context = C> + Default,
    C: Send + Sync + 'static,
{
    Box::pin(async move {
        let ctx: Arc<C> = ctx
            .downcast::<C>()
            .unwrap_or_else(|_| panic!("message_worker '{}' context type mismatch", W::NAME));
        W::default().drain(ctx).await
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tokio::sync::{mpsc, Semaphore};
    use tokio::time::{timeout, Instant};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Job {
        key: u32,
        tag: &'static str,
    }

    fn job(key: u32, tag: &'static str) -> Job {
        Job { key, tag }
    }

    const FAILING: u32 = 99;
    const SETTLE: Duration = Duration::from_secs(5);
    const RETRY_DELAY: Duration = Duration::from_secs(1);
    const QUIET: Duration = Duration::from_secs(60);

    struct Ctx {
        sender: MessageSender<Recorder>,
        #[cfg(feature = "k8s")]
        leader: Option<tokio::sync::watch::Sender<bool>>,
    }

    #[cfg(feature = "k8s")]
    impl crate::k8s::HasLeaderElection for Ctx {
        fn is_leader_rx(&self) -> tokio::sync::watch::Receiver<bool> {
            self.leader.as_ref().expect("HA context").subscribe()
        }

        fn ha_enabled(&self) -> bool {
            self.leader.is_some()
        }
    }

    /// Reports each message as it starts, then holds it until the test
    /// releases a permit.
    struct Recorder {
        started: mpsc::UnboundedSender<Job>,
        permits: Arc<Semaphore>,
        retries: Arc<Mutex<HashMap<u32, u32>>>,
    }

    fn take_retry(retries: &Mutex<HashMap<u32, u32>>, key: u32) -> bool {
        let mut retries = retries.lock().unwrap();
        let Some(left) = retries.get_mut(&key).filter(|left| **left > 0) else {
            return false;
        };
        *left -= 1;
        true
    }

    impl MessageWorker for Recorder {
        type Message = Job;
        type Key = u32;
        type Context = Ctx;
        type Error = String;

        const NAME: &'static str = "recorder";
        const CAPACITY: usize = 3;
        const SETTLE: Duration = SETTLE;
        const RETRY_DELAY: Duration = RETRY_DELAY;

        fn key(msg: &Job) -> u32 {
            msg.key
        }

        fn sender(ctx: &Ctx) -> &MessageSender<Self> {
            &ctx.sender
        }

        fn handle(&self, _ctx: Arc<Ctx>, msg: Job) -> BoxFuture<Result<MessageResult, String>> {
            let started = self.started.clone();
            let permits = Arc::clone(&self.permits);
            let retries = Arc::clone(&self.retries);
            Box::pin(async move {
                started.send(msg.clone()).unwrap();
                permits.acquire().await.unwrap().forget();
                if msg.key == FAILING {
                    return Err("boom".to_string());
                }
                if take_retry(&retries, msg.key) {
                    return Ok(MessageResult::Retry("again".to_string()));
                }
                Ok(MessageResult::Done)
            })
        }
    }

    struct Harness {
        sender: MessageSender<Recorder>,
        started: mpsc::UnboundedReceiver<Job>,
        permits: Arc<Semaphore>,
        retries: Arc<Mutex<HashMap<u32, u32>>>,
        #[cfg(feature = "k8s")]
        leader: Option<tokio::sync::watch::Sender<bool>>,
    }

    impl Harness {
        fn standalone(permits: usize) -> Self {
            Self::new(permits, None)
        }

        #[cfg(feature = "k8s")]
        fn ha(permits: usize, leading: bool) -> Self {
            Self::new(permits, Some(leading))
        }

        #[cfg_attr(not(feature = "k8s"), allow(unused_variables))]
        fn new(permits: usize, leading: Option<bool>) -> Self {
            let (started_tx, started) = mpsc::unbounded_channel();
            let harness = Self {
                sender: MessageSender::new(),
                started,
                permits: Arc::new(Semaphore::new(permits)),
                retries: Arc::default(),
                #[cfg(feature = "k8s")]
                leader: leading.map(|leading| tokio::sync::watch::channel(leading).0),
            };
            let ctx = Ctx {
                sender: harness.sender.clone(),
                #[cfg(feature = "k8s")]
                leader: harness.leader.clone(),
            };
            let worker = Recorder {
                started: started_tx,
                permits: Arc::clone(&harness.permits),
                retries: Arc::clone(&harness.retries),
            };
            tokio::spawn(serve(worker, Arc::new(ctx)));
            harness
        }

        fn send(&self, key: u32, tag: &'static str) {
            self.sender.send(job(key, tag)).unwrap();
        }

        async fn next_started(&mut self) -> Job {
            timeout(QUIET, self.started.recv())
                .await
                .expect("a message started")
                .unwrap()
        }

        async fn assert_quiet(&mut self, within: Duration) {
            if let Ok(job) = timeout(within, self.started.recv()).await {
                panic!("nothing should start, but {job:?} did");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn messages_are_handled_in_send_order_one_at_a_time() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");

        assert_eq!(h.next_started().await, job(1, "a"));
        h.assert_quiet(QUIET).await;
        h.permits.add_permits(1);
        assert_eq!(h.next_started().await, job(2, "b"));
        h.permits.add_permits(1);
        assert_eq!(h.next_started().await, job(3, "c"));
    }

    #[tokio::test(start_paused = true)]
    async fn resending_a_queued_key_keeps_its_place_and_payload() {
        let mut h = Harness::standalone(10);
        h.send(1, "first");
        h.send(2, "b");
        h.send(1, "second");

        assert_eq!(h.sender.position(&1), Some(1));
        assert_eq!(h.sender.position(&2), Some(2));
        assert_eq!(h.next_started().await, job(1, "first"));
        assert_eq!(h.next_started().await, job(2, "b"));
        h.assert_quiet(QUIET).await;
    }

    #[tokio::test(start_paused = true)]
    async fn resending_the_key_being_handled_queues_it_again() {
        let mut h = Harness::standalone(0);
        h.send(1, "first");
        assert_eq!(h.next_started().await, job(1, "first"));

        h.send(1, "second");
        assert_eq!(h.sender.position(&1), Some(0));
        h.permits.add_permits(1);
        assert_eq!(h.next_started().await, job(1, "second"));
    }

    #[tokio::test(start_paused = true)]
    async fn position_counts_from_the_message_being_handled() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");
        assert_eq!(h.sender.position(&1), Some(1));
        assert_eq!(h.sender.position(&3), Some(3));
        assert_eq!(h.sender.position(&4), None);

        h.next_started().await;
        assert_eq!(h.sender.position(&1), Some(0));
        assert_eq!(h.sender.position(&2), Some(1));
        assert_eq!(h.sender.position(&3), Some(2));

        h.permits.add_permits(1);
        h.next_started().await;
        assert_eq!(h.sender.position(&1), None);
        assert_eq!(h.sender.position(&2), Some(0));
        assert_eq!(h.sender.position(&3), Some(1));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_moves_the_message_to_the_back_after_the_retry_delay() {
        let mut h = Harness::standalone(10);
        h.retries.lock().unwrap().insert(1, 1);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");

        assert_eq!(h.next_started().await, job(1, "a"));
        let retried_at = Instant::now();
        assert_eq!(h.next_started().await, job(2, "b"));
        assert!(retried_at.elapsed() >= RETRY_DELAY);
        assert_eq!(h.next_started().await, job(3, "c"));
        assert_eq!(h.next_started().await, job(1, "a"));
        h.assert_quiet(QUIET).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_gives_way_to_a_resend_of_its_key() {
        let mut h = Harness::standalone(0);
        h.retries.lock().unwrap().insert(1, 1);
        h.send(1, "first");
        assert_eq!(h.next_started().await, job(1, "first"));
        h.send(1, "second");
        h.send(2, "b");

        h.permits.add_permits(10);
        assert_eq!(h.next_started().await, job(1, "second"));
        assert_eq!(h.next_started().await, job(2, "b"));
        h.assert_quiet(QUIET).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_message_is_dropped_and_the_worker_keeps_going() {
        let mut h = Harness::standalone(10);
        h.send(FAILING, "boom");
        h.send(1, "a");

        assert_eq!(h.next_started().await, job(FAILING, "boom"));
        assert_eq!(h.next_started().await, job(1, "a"));
        h.assert_quiet(QUIET).await;
        h.send(2, "b");
        assert_eq!(h.next_started().await, job(2, "b"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_key_is_refused_when_full_and_coalescing_still_succeeds() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");
        let full = QueueFull {
            worker: "recorder",
            capacity: 3,
        };
        assert_eq!(h.sender.send(job(4, "d")), Err(full));
        assert_eq!(h.sender.send(job(2, "again")), Ok(()));

        h.next_started().await;
        assert_eq!(h.sender.send(job(4, "d")), Err(full));

        h.permits.add_permits(1);
        h.next_started().await;
        assert_eq!(h.sender.send(job(4, "d")), Ok(()));
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn nothing_is_handled_while_not_leader() {
        let mut h = Harness::ha(10, false);
        h.send(1, "a");

        h.assert_quiet(QUIET).await;
        assert_eq!(h.sender.position(&1), Some(1));
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn the_first_message_waits_for_settle_after_becoming_leader() {
        let mut h = Harness::ha(10, false);
        h.send(1, "a");
        h.assert_quiet(QUIET).await;

        h.leader.as_ref().unwrap().send(true).unwrap();
        h.assert_quiet(SETTLE - Duration::from_millis(1)).await;
        assert_eq!(
            timeout(Duration::from_millis(2), h.started.recv()).await,
            Ok(Some(job(1, "a")))
        );
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn losing_leadership_drops_the_queue() {
        let mut h = Harness::ha(0, true);
        h.send(1, "a");
        h.send(2, "b");
        assert_eq!(h.next_started().await, job(1, "a"));

        h.leader.as_ref().unwrap().send(false).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(h.sender.position(&1), None);
        assert_eq!(h.sender.position(&2), None);

        h.leader.as_ref().unwrap().send(true).unwrap();
        h.permits.add_permits(10);
        h.assert_quiet(QUIET).await;
        h.send(3, "c");
        assert_eq!(h.next_started().await, job(3, "c"));
    }
}
