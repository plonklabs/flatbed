//! Local queue worker: a bounded, keyed, in-process queue handled one message
//! at a time.
//!
//! The application context owns one queue per [`LocalQueueWorker`] type
//! ([`HasLocalQueues`]). Producers inside the process enqueue with
//! `ctx.local_queue::<W>().send(msg)`; the worker takes the messages strictly
//! in order, taking the next only after [`handle`](LocalQueueWorker::handle)
//! returns. Nothing is persisted: the queue is lost on restart, so producers
//! are expected to be level-triggered and resend what they still need.
//!
//! ## Queue semantics
//!
//! - **Coalescing.** Every message has a [`key`](LocalQueueWorker::key). Sending
//!   a key that is already queued adds nothing: the queued entry keeps its
//!   place in line, its original payload and its attempt count, and the new
//!   payload is discarded. The message being handled counts as *not* queued,
//!   so a resend during handling queues the key again, to run after the
//!   current attempt.
//! - **Status.** [`LocalQueueSender::status`] reports a key as
//!   [`InHand`](KeyStatus::InHand) while it is being handled,
//!   [`Queued(1)`](KeyStatus::Queued) when it is next,
//!   [`DeadLettered`](KeyStatus::DeadLettered) while it is set aside, and
//!   [`Absent`](KeyStatus::Absent) otherwise.
//! - **Capacity.** At most [`CAPACITY`](LocalQueueWorker::CAPACITY) messages are
//!   held, counting the one being handled. Sending a new key into a full
//!   queue returns [`SendError::Full`]; coalescing into a queued key always
//!   succeeds.
//! - **Attempts.** Every call to `handle` is an attempt, counted per key. A
//!   key's count survives a retry and a coalesced resend, and is cleared when
//!   the key is done or set aside.
//! - **Outcome.** [`LocalQueueResult::Done`] finishes the message.
//!   [`LocalQueueResult::Retry`] puts it back at the end of the line, unless
//!   the same key was resent meanwhile, in which case the queued resend
//!   stands in for it and takes over its count. After a retry the worker
//!   waits [`RETRY_DELAY`](LocalQueueWorker::RETRY_DELAY) before taking the
//!   next message, so messages that keep failing never spin. A retry on the
//!   [`MAX_ATTEMPTS`](LocalQueueWorker::MAX_ATTEMPTS)th attempt, an `Err`, a
//!   panic, or a `handle` outrunning
//!   [`HANDLE_TIMEOUT`](LocalQueueWorker::HANDLE_TIMEOUT) sets the message
//!   aside as a dead letter. None of these stop the worker or touch health.
//! - **Dead letters.** A message set aside joins the dead-letter set, kept by
//!   key, and any queued resend of its key is dropped. The worker's
//!   [`dead_letter`](LocalQueueWorker::dead_letter) hook then runs in the
//!   line: the next message waits for it, under the same `HANDLE_TIMEOUT`.
//!   While a key is set aside, sending it returns [`SendError::DeadLettered`];
//!   [`requeue`](LocalQueueSender::requeue) puts it back at the end of the
//!   line with its count reset, and [`forget`](LocalQueueSender::forget)
//!   drops it. Past [`DEAD_LETTER_CAPACITY`](LocalQueueWorker::DEAD_LETTER_CAPACITY)
//!   the oldest is evicted, and its key can be sent again.
//!
//! ## Leadership
//!
//! With the `k8s` feature the context provides leader election (see
//! [`LocalQueueWorker::Context`]), and messages are handled only while this
//! process leads. Each time it becomes leader the worker waits
//! [`SETTLE`](LocalQueueWorker::SETTLE) before the first message, so a request
//! the previous leader already sent lands first. When it loses leadership the
//! message being handled, or the `dead_letter` hook running, is cancelled, and
//! the whole queue, the attempt counts and the dead-letter set are discarded.
//! Sends are accepted whether or not the process leads; what a follower
//! queues waits for the next leadership term, and is discarded with the rest
//! if leadership is lost first. With leader election disabled
//! (`ha_enabled() == false`), or without the `k8s` feature, the worker
//! handles messages from startup, after one `SETTLE`.

use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::num::NonZeroU32;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use futures::FutureExt;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::error_reporting::{caught_panic, report_panic, PanicSite};
use crate::{BoxFuture, FlatbedWorkerError};

/// Outcome of handling one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalQueueResult {
    /// The message is finished.
    Done,
    /// Put the message back at the end of the line; the reason is logged.
    /// Use it for an unmet precondition, such as a server that is not ready,
    /// never to poll work that is still running: the next message starts as
    /// soon as `handle` returns. It counts as an attempt.
    Retry(String),
}

/// A message set aside: why, and after how many attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetter {
    /// Why the message was set aside.
    pub cause: DeadLetterCause,
    /// How many times `handle` ran for the key.
    pub attempts: u32,
    /// The reason the latest `Retry` or `Err` for the key carried.
    pub last_reason: Option<String>,
    /// When the message was set aside.
    pub at: SystemTime,
}

/// Why a message was set aside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeadLetterCause {
    /// `handle` returned `Retry` on the
    /// [`MAX_ATTEMPTS`](LocalQueueWorker::MAX_ATTEMPTS)th attempt.
    Exhausted,
    /// `handle` ran longer than
    /// [`HANDLE_TIMEOUT`](LocalQueueWorker::HANDLE_TIMEOUT) and was dropped.
    TimedOut {
        /// The timeout it ran into.
        after: Duration,
    },
    /// `handle` returned `Err`, carrying its `Display`, or panicked, carrying
    /// `"panicked: "` and the panic message.
    Failed(String),
}

/// Where a key stands, from [`LocalQueueSender::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    /// Being handled.
    InHand,
    /// Waiting in line; `Queued(1)` is next.
    Queued(usize),
    /// Set aside; see [`LocalQueueSender::requeue`] and
    /// [`LocalQueueSender::forget`].
    DeadLettered(DeadLetter),
    /// Neither handled, queued nor set aside.
    Absent,
}

/// Worker handling a keyed, in-process queue one message at a time.
///
/// See the [module docs](self) for the queue, dead-letter and leadership
/// semantics.
///
/// # Requirements
///
/// Implementors must also implement `Default`: the runtime executor builds
/// the worker with `W::default()` each time it starts.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::local_queue::{DeadLetter, HasLocalQueues, LocalQueueResult, LocalQueueWorker};
/// use std::num::NonZeroU32;
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// type AppContext = flatbed::FlatbedContext<AppData>;
///
/// #[derive(Clone)]
/// struct RunMigrator { server: String, migrator: String }
///
/// #[derive(Default)]
/// struct MigratorRunner;
///
/// impl LocalQueueWorker for MigratorRunner {
///     type Message = RunMigrator;
///     type Key = (String, String);
///     type Context = AppContext;
///     type Error = kube::Error;
///
///     const NAME: &'static str = "migrator-runner";
///     const MAX_ATTEMPTS: Option<NonZeroU32> = NonZeroU32::new(5);
///     const HANDLE_TIMEOUT: Option<Duration> = Some(Duration::from_secs(900));
///
///     fn key(msg: &RunMigrator) -> Self::Key {
///         (msg.server.clone(), msg.migrator.clone())
///     }
///
///     fn handle(
///         &self,
///         ctx: Arc<AppContext>,
///         msg: RunMigrator,
///     ) -> flatbed::BoxFuture<Result<LocalQueueResult, kube::Error>> {
///         Box::pin(async move {
///             if !server_ready(&ctx, &msg.server).await? {
///                 return Ok(LocalQueueResult::Retry("server not ready".into()));
///             }
///             run_job_to_completion(&ctx, &msg).await?;
///             Ok(LocalQueueResult::Done)
///         })
///     }
///
///     fn dead_letter(
///         &self,
///         ctx: Arc<AppContext>,
///         msg: RunMigrator,
///         _letter: DeadLetter,
///     ) -> flatbed::BoxFuture<()> {
///         Box::pin(async move { delete_job(&ctx, &msg).await })
///     }
/// }
///
/// flatbed::register_local_queue_worker!(MigratorRunner, AppContext);
///
/// // In a reconciler holding `ctx: Arc<AppContext>`:
/// ctx.local_queue::<MigratorRunner>().send(RunMigrator { server, migrator })?;
/// let status = ctx.local_queue::<MigratorRunner>().status(&key);
/// ```
pub trait LocalQueueWorker: Sized + Send + Sync + 'static {
    /// The message type carried by the queue. Cloned once per attempt so a
    /// retry can re-enqueue it.
    type Message: Clone + Send + 'static;

    /// Identity used to coalesce messages; see [`key`](Self::key).
    type Key: Eq + Send + 'static;

    /// The application context type owning the queue and providing leader
    /// election.
    #[cfg(feature = "k8s")]
    type Context: HasLocalQueues + crate::k8s::HasLeaderElection + Send + Sync + 'static;

    /// The application context type owning the queue.
    #[cfg(not(feature = "k8s"))]
    type Context: HasLocalQueues + Send + Sync + 'static;

    /// Error returned by [`handle`](Self::handle). The runtime sets the
    /// message aside as [`DeadLetterCause::Failed`] with its `Display`.
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

    /// Pause after becoming leader before the first message is taken, so a
    /// request the previous leader already sent lands before this leader
    /// issues its first one. Defaults to zero.
    const SETTLE: Duration = Duration::ZERO;

    /// Pause after a [`LocalQueueResult::Retry`] before the next message is
    /// taken.
    const RETRY_DELAY: Duration = Duration::from_secs(1);

    /// Most times `handle` runs for one key before a `Retry` sets it aside as
    /// [`DeadLetterCause::Exhausted`]. `None`, the default, retries without
    /// limit.
    const MAX_ATTEMPTS: Option<NonZeroU32> = None;

    /// Longest one [`handle`](Self::handle) may run before it is dropped and
    /// the message set aside as [`DeadLetterCause::TimedOut`]. It also bounds
    /// [`dead_letter`](Self::dead_letter). `None`, the default, sets no limit.
    const HANDLE_TIMEOUT: Option<Duration> = None;

    /// Most messages kept set aside; past it the oldest is evicted.
    const DEAD_LETTER_CAPACITY: usize = 256;

    /// The coalescing key of a message: two messages with equal keys are the
    /// same request.
    fn key(msg: &Self::Message) -> Self::Key;

    /// Handle one message. The next message is taken only after the
    /// returned future completes. The future is dropped mid-flight when
    /// leadership is lost or `HANDLE_TIMEOUT` runs out.
    fn handle(
        &self,
        ctx: Arc<Self::Context>,
        msg: Self::Message,
    ) -> BoxFuture<Result<LocalQueueResult, Self::Error>>;

    /// Called with each message set aside, after it has joined the
    /// dead-letter set. The next message waits for the returned future,
    /// under `HANDLE_TIMEOUT`. Best effort: a leadership loss cancels it, as
    /// it cancels `handle`. Defaults to logging the letter at error.
    fn dead_letter(
        &self,
        _ctx: Arc<Self::Context>,
        _msg: Self::Message,
        letter: DeadLetter,
    ) -> BoxFuture<()> {
        Box::pin(async move {
            error!(
                worker = Self::NAME,
                cause = ?letter.cause,
                attempts = letter.attempts,
                last_reason = ?letter.last_reason,
                "message dead-lettered"
            );
        })
    }

    /// Finish in-progress work during graceful shutdown, within the
    /// configured shutdown budget. Defaults to a no-op.
    fn drain(&self, _ctx: Arc<Self::Context>) -> BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Sending a new key into a queue already holding
/// [`LocalQueueWorker::CAPACITY`] messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalQueueFull {
    /// The worker's [`LocalQueueWorker::NAME`].
    pub worker: &'static str,
    /// The worker's [`LocalQueueWorker::CAPACITY`].
    pub capacity: usize,
}

impl fmt::Display for LocalQueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "local queue worker '{}' is full ({} messages)",
            self.worker, self.capacity
        )
    }
}

impl std::error::Error for LocalQueueFull {}

/// Why [`LocalQueueSender::send`] refused a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SendError {
    /// The key is new and the queue is full.
    Full(LocalQueueFull),
    /// The key is set aside; [`LocalQueueSender::requeue`] brings it back.
    DeadLettered,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(full) => full.fmt(f),
            Self::DeadLettered => {
                f.write_str("the key is dead-lettered; requeue it to send it again")
            }
        }
    }
}

impl std::error::Error for SendError {}

impl From<LocalQueueFull> for SendError {
    fn from(full: LocalQueueFull) -> Self {
        Self::Full(full)
    }
}

/// Contexts that own the local queues: one queue per [`LocalQueueWorker`]
/// type, shared by every clone of the context.
///
/// [`FlatbedContext`](crate::FlatbedContext) implements it. A hand-rolled
/// context holds a [`LocalQueues`] and returns it.
pub trait HasLocalQueues {
    /// The context's queues.
    fn local_queues(&self) -> &LocalQueues;

    /// The queue for worker `W`, created on first use. Producers and the
    /// worker get the same queue whichever of them asks first.
    fn local_queue<W: LocalQueueWorker>(&self) -> LocalQueueSender<W> {
        self.local_queues().get::<W>()
    }
}

/// The per-worker-type queues behind [`HasLocalQueues`]. Clones share the
/// same queues.
#[derive(Clone, Default)]
pub struct LocalQueues {
    queues: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

impl LocalQueues {
    fn get<W: LocalQueueWorker>(&self) -> LocalQueueSender<W> {
        let mut queues = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
        queues
            .entry(TypeId::of::<W>())
            .or_insert_with(|| Box::new(LocalQueueSender::<W>::new()))
            .downcast_ref::<LocalQueueSender<W>>()
            .expect("local queues are keyed by worker type")
            .clone()
    }
}

impl fmt::Debug for LocalQueues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalQueues").finish_non_exhaustive()
    }
}

/// Enqueues messages for a [`LocalQueueWorker`]; obtained from
/// [`HasLocalQueues::local_queue`]. Clones share one queue and are cheap.
/// Sending never waits for the message to be handled.
pub struct LocalQueueSender<W: LocalQueueWorker> {
    shared: Arc<Shared<W>>,
}

struct Shared<W: LocalQueueWorker> {
    state: Mutex<State<W>>,
    wake: Notify,
}

struct Entry<W: LocalQueueWorker> {
    key: W::Key,
    msg: W::Message,
    attempts: u32,
    last_reason: Option<String>,
}

impl<W: LocalQueueWorker> Entry<W> {
    fn new(key: W::Key, msg: W::Message) -> Self {
        Self {
            key,
            msg,
            attempts: 0,
            last_reason: None,
        }
    }
}

struct InHand<W: LocalQueueWorker> {
    key: W::Key,
    attempts: u32,
    last_reason: Option<String>,
}

struct Aside<W: LocalQueueWorker> {
    key: W::Key,
    msg: W::Message,
    letter: DeadLetter,
}

struct State<W: LocalQueueWorker> {
    queued: VecDeque<Entry<W>>,
    in_flight: Option<InHand<W>>,
    dead: VecDeque<Aside<W>>,
}

impl<W: LocalQueueWorker> State<W> {
    fn queued_index(&self, key: &W::Key) -> Option<usize> {
        self.queued.iter().position(|entry| &entry.key == key)
    }

    fn dead_index(&self, key: &W::Key) -> Option<usize> {
        self.dead.iter().position(|aside| &aside.key == key)
    }

    fn has_room(&self) -> Result<(), LocalQueueFull> {
        if self.queued.len() + usize::from(self.in_flight.is_some()) < W::CAPACITY {
            return Ok(());
        }
        Err(LocalQueueFull {
            worker: W::NAME,
            capacity: W::CAPACITY,
        })
    }

    fn set_aside(
        &mut self,
        hand: InHand<W>,
        msg: W::Message,
        cause: DeadLetterCause,
    ) -> DeadLetter {
        if let Some(index) = self.queued_index(&hand.key) {
            self.queued.remove(index);
        }
        let letter = DeadLetter {
            cause,
            attempts: hand.attempts,
            last_reason: hand.last_reason,
            at: SystemTime::now(),
        };
        self.dead.push_back(Aside {
            key: hand.key,
            msg,
            letter: letter.clone(),
        });
        while self.dead.len() > W::DEAD_LETTER_CAPACITY {
            let Some(evicted) = self.dead.pop_front() else {
                break;
            };
            warn!(
                worker = W::NAME,
                capacity = W::DEAD_LETTER_CAPACITY,
                cause = ?evicted.letter.cause,
                "dead-letter set full, evicted the oldest"
            );
        }
        letter
    }
}

impl<W: LocalQueueWorker> LocalQueueSender<W> {
    fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queued: VecDeque::new(),
                    in_flight: None,
                    dead: VecDeque::new(),
                }),
                wake: Notify::new(),
            }),
        }
    }

    /// Enqueue `msg` at the end of the line, or coalesce it into the queued
    /// message with the same key, which keeps its place, payload and count.
    ///
    /// # Errors
    ///
    /// [`SendError::DeadLettered`] when the key is set aside, and
    /// [`SendError::Full`] when the key is not queued and the queue already
    /// holds [`LocalQueueWorker::CAPACITY`] messages.
    pub fn send(&self, msg: W::Message) -> Result<(), SendError> {
        let key = W::key(&msg);
        let mut state = self.lock();
        if state.dead_index(&key).is_some() {
            return Err(SendError::DeadLettered);
        }
        if state.queued_index(&key).is_some() {
            return Ok(());
        }
        state.has_room()?;
        state.queued.push_back(Entry::new(key, msg));
        drop(state);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Where `key` stands. A key resent during its own handling reports
    /// [`KeyStatus::InHand`] until that attempt finishes.
    #[must_use]
    pub fn status(&self, key: &W::Key) -> KeyStatus {
        let state = self.lock();
        if state
            .in_flight
            .as_ref()
            .is_some_and(|hand| &hand.key == key)
        {
            return KeyStatus::InHand;
        }
        if let Some(index) = state.queued_index(key) {
            return KeyStatus::Queued(index + 1);
        }
        state.dead_index(key).map_or(KeyStatus::Absent, |index| {
            KeyStatus::DeadLettered(state.dead[index].letter.clone())
        })
    }

    /// Put a set-aside key back at the end of the line with its attempt
    /// count reset. A key that is not set aside is left as it is.
    ///
    /// # Errors
    ///
    /// [`LocalQueueFull`] when the queue already holds
    /// [`LocalQueueWorker::CAPACITY`] messages; the key stays set aside.
    pub fn requeue(&self, key: &W::Key) -> Result<(), LocalQueueFull> {
        let mut state = self.lock();
        let Some(index) = state.dead_index(key) else {
            return Ok(());
        };
        state.has_room()?;
        let Some(aside) = state.dead.remove(index) else {
            return Ok(());
        };
        state.queued.push_back(Entry::new(aside.key, aside.msg));
        drop(state);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Drop a set-aside key, so it can be sent again. Returns whether it was
    /// set aside.
    pub fn forget(&self, key: &W::Key) -> bool {
        let mut state = self.lock();
        let Some(index) = state.dead_index(key) else {
            return false;
        };
        state.dead.remove(index);
        true
    }

    fn lock(&self) -> MutexGuard<'_, State<W>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn next(&self) -> (W::Message, u32) {
        loop {
            let entry = {
                let mut state = self.lock();
                state.queued.pop_front().map(|entry| {
                    let attempts = entry.attempts.saturating_add(1);
                    state.in_flight = Some(InHand {
                        key: entry.key,
                        attempts,
                        last_reason: entry.last_reason,
                    });
                    (entry.msg, attempts)
                })
            };
            if let Some(entry) = entry {
                return entry;
            }
            self.shared.wake.notified().await;
        }
    }

    fn retry(&self, msg: W::Message, reason: String) -> Option<DeadLetter> {
        let mut state = self.lock();
        let mut hand = state.in_flight.take()?;
        hand.last_reason = Some(reason);
        if W::MAX_ATTEMPTS.is_some_and(|max| hand.attempts >= max.get()) {
            return Some(state.set_aside(hand, msg, DeadLetterCause::Exhausted));
        }
        let Some(index) = state.queued_index(&hand.key) else {
            let mut entry = Entry::new(hand.key, msg);
            entry.attempts = hand.attempts;
            entry.last_reason = hand.last_reason;
            state.queued.push_back(entry);
            return None;
        };
        let resend = &mut state.queued[index];
        resend.attempts = hand.attempts;
        resend.last_reason = hand.last_reason;
        None
    }

    fn fail(&self, msg: W::Message, cause: DeadLetterCause) -> Option<DeadLetter> {
        let mut state = self.lock();
        let mut hand = state.in_flight.take()?;
        if let DeadLetterCause::Failed(reason) = &cause {
            hand.last_reason = Some(reason.clone());
        }
        Some(state.set_aside(hand, msg, cause))
    }

    fn release_in_flight(&self) {
        self.lock().in_flight = None;
    }

    #[cfg(feature = "k8s")]
    fn clear(&self) -> (usize, usize) {
        let mut state = self.lock();
        let dropped = state.queued.len() + usize::from(state.in_flight.is_some());
        let dead = state.dead.len();
        state.queued.clear();
        state.in_flight = None;
        state.dead.clear();
        (dropped, dead)
    }
}

impl<W: LocalQueueWorker> Clone for LocalQueueSender<W> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<W: LocalQueueWorker> fmt::Debug for LocalQueueSender<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalQueueSender")
            .field("worker", &W::NAME)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Runtime Executor
// ============================================================================

/// Run a [`LocalQueueWorker`]: the executor behind `register_local_queue_worker!`.
pub async fn run_local_queue_worker<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    W: LocalQueueWorker<Context = C> + Default,
    C: Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("local_queue_worker '{}' context type mismatch", W::NAME));
    serve(W::default(), ctx).await
}

#[cfg(feature = "k8s")]
async fn serve<W: LocalQueueWorker>(
    worker: W,
    ctx: Arc<W::Context>,
) -> Result<(), FlatbedWorkerError> {
    use crate::k8s::{wait_for_leadership, wait_for_leadership_loss, HasLeaderElection};

    let sender = ctx.local_queue::<W>();
    sender.release_in_flight();

    if !ctx.ha_enabled() {
        info!(worker = W::NAME, "standalone mode, handling messages");
        handle_messages(&worker, &ctx, &sender).await;
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
            () = handle_messages(&worker, &ctx, &sender) => {}
            () = wait_for_leadership_loss(&mut loss_rx, W::NAME) => {}
        }
        let (dropped, dead_letters) = sender.clear();
        info!(
            worker = W::NAME,
            dropped, dead_letters, "lost leadership, dropped queued and dead-lettered messages"
        );
    }
}

#[cfg(not(feature = "k8s"))]
async fn serve<W: LocalQueueWorker>(
    worker: W,
    ctx: Arc<W::Context>,
) -> Result<(), FlatbedWorkerError> {
    let sender = ctx.local_queue::<W>();
    sender.release_in_flight();
    info!(worker = W::NAME, "handling messages");
    handle_messages(&worker, &ctx, &sender).await;
    Ok(())
}

/// Settle, then handle messages until cancelled. Never returns.
async fn handle_messages<W: LocalQueueWorker>(
    worker: &W,
    ctx: &Arc<W::Context>,
    sender: &LocalQueueSender<W>,
) {
    tokio::time::sleep(W::SETTLE).await;
    loop {
        let (msg, attempts) = sender.next().await;
        let letter = match attempt(worker, ctx, msg.clone()).await {
            Ok(LocalQueueResult::Done) => {
                debug!(worker = W::NAME, "message done");
                sender.release_in_flight();
                continue;
            }
            Ok(LocalQueueResult::Retry(reason)) => {
                let letter = sender.retry(msg.clone(), reason.clone());
                if letter.is_none() {
                    warn!(worker = W::NAME, reason = %reason, attempts, "retrying message");
                    tokio::time::sleep(W::RETRY_DELAY).await;
                }
                letter
            }
            Err(cause) => sender.fail(msg.clone(), cause),
        };
        let Some(letter) = letter else {
            continue;
        };
        dead_letter(worker, ctx, msg, letter).await;
    }
}

async fn attempt<W: LocalQueueWorker>(
    worker: &W,
    ctx: &Arc<W::Context>,
    msg: W::Message,
) -> Result<LocalQueueResult, DeadLetterCause> {
    let handled = AssertUnwindSafe(worker.handle(Arc::clone(ctx), msg)).catch_unwind();
    let result = match W::HANDLE_TIMEOUT {
        Some(after) => tokio::time::timeout(after, handled)
            .await
            .map_err(|_| DeadLetterCause::TimedOut { after })?,
        None => handled.await,
    };
    match result {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(e)) => Err(DeadLetterCause::Failed(e.to_string())),
        Err(payload) => {
            let panic = caught_panic(payload.as_ref());
            report_panic(PanicSite::Worker(W::NAME), &panic);
            Err(DeadLetterCause::Failed(format!("panicked: {panic}")))
        }
    }
}

async fn dead_letter<W: LocalQueueWorker>(
    worker: &W,
    ctx: &Arc<W::Context>,
    msg: W::Message,
    letter: DeadLetter,
) {
    let hook = AssertUnwindSafe(worker.dead_letter(Arc::clone(ctx), msg, letter)).catch_unwind();
    let result = match W::HANDLE_TIMEOUT {
        Some(after) => tokio::time::timeout(after, hook).await,
        None => Ok(hook.await),
    };
    match result {
        Ok(Ok(())) => {}
        Ok(Err(payload)) => {
            let panic = caught_panic(payload.as_ref());
            report_panic(PanicSite::Worker(W::NAME), &panic);
            error!(worker = W::NAME, panic = %panic, "dead_letter hook panicked");
        }
        Err(_) => warn!(worker = W::NAME, "dead_letter hook timed out, moving on"),
    }
}

/// Drain a [`LocalQueueWorker`] by downcasting the context and delegating to
/// `LocalQueueWorker::drain`.
pub fn run_local_queue_worker_drain<W, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> BoxFuture<Result<(), FlatbedWorkerError>>
where
    W: LocalQueueWorker<Context = C> + Default,
    C: Send + Sync + 'static,
{
    Box::pin(async move {
        let ctx: Arc<C> = ctx
            .downcast::<C>()
            .unwrap_or_else(|_| panic!("local_queue_worker '{}' context type mismatch", W::NAME));
        W::default().drain(ctx).await
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{mpsc, Semaphore};
    use tokio::task::JoinHandle;
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

    const SETTLE: Duration = Duration::from_secs(5);
    const RETRY_DELAY: Duration = Duration::from_secs(1);
    const HANDLE_TIMEOUT: Duration = Duration::from_secs(300);
    const QUIET: Duration = Duration::from_secs(60);

    #[derive(Clone)]
    struct Ctx {
        queues: LocalQueues,
        #[cfg(feature = "k8s")]
        leader: Option<tokio::sync::watch::Sender<bool>>,
    }

    impl HasLocalQueues for Ctx {
        fn local_queues(&self) -> &LocalQueues {
            &self.queues
        }
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
    /// releases a permit. A handle future dropped before it finishes counts
    /// as cancelled. A message tagged `err` fails and one tagged `panic`
    /// panics. The dead-letter hook reports each letter, then holds the line
    /// until it gets a hook permit.
    #[derive(Clone)]
    struct Recorder {
        started: mpsc::UnboundedSender<Job>,
        letters: mpsc::UnboundedSender<(Job, DeadLetter)>,
        permits: Arc<Semaphore>,
        hook_permits: Arc<Semaphore>,
        retries: Arc<Mutex<HashMap<u32, u32>>>,
        cancelled: Arc<AtomicUsize>,
    }

    struct CancelGuard(Arc<AtomicUsize>);

    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn take_retry(retries: &Mutex<HashMap<u32, u32>>, key: u32) -> bool {
        let mut retries = retries.lock().unwrap();
        let Some(left) = retries.get_mut(&key).filter(|left| **left > 0) else {
            return false;
        };
        *left -= 1;
        true
    }

    impl LocalQueueWorker for Recorder {
        type Message = Job;
        type Key = u32;
        type Context = Ctx;
        type Error = String;

        const NAME: &'static str = "recorder";
        const CAPACITY: usize = 3;
        const SETTLE: Duration = SETTLE;
        const RETRY_DELAY: Duration = RETRY_DELAY;
        const MAX_ATTEMPTS: Option<NonZeroU32> = NonZeroU32::new(3);
        const HANDLE_TIMEOUT: Option<Duration> = Some(HANDLE_TIMEOUT);
        const DEAD_LETTER_CAPACITY: usize = 2;

        fn key(msg: &Job) -> u32 {
            msg.key
        }

        fn handle(&self, _ctx: Arc<Ctx>, msg: Job) -> BoxFuture<Result<LocalQueueResult, String>> {
            let started = self.started.clone();
            let permits = Arc::clone(&self.permits);
            let retries = Arc::clone(&self.retries);
            let guard = CancelGuard(Arc::clone(&self.cancelled));
            Box::pin(async move {
                started.send(msg.clone()).unwrap();
                permits.acquire().await.unwrap().forget();
                std::mem::forget(guard);
                match msg.tag {
                    "err" => return Err("boom".to_string()),
                    "panic" => panic!("kaboom"),
                    _ => {}
                }
                if take_retry(&retries, msg.key) {
                    return Ok(LocalQueueResult::Retry("again".to_string()));
                }
                Ok(LocalQueueResult::Done)
            })
        }

        fn dead_letter(&self, _ctx: Arc<Ctx>, msg: Job, letter: DeadLetter) -> BoxFuture<()> {
            let letters = self.letters.clone();
            let hook_permits = Arc::clone(&self.hook_permits);
            Box::pin(async move {
                letters.send((msg, letter)).unwrap();
                hook_permits.acquire().await.unwrap().forget();
            })
        }
    }

    struct Harness {
        ctx: Ctx,
        worker: Recorder,
        started: mpsc::UnboundedReceiver<Job>,
        letters: mpsc::UnboundedReceiver<(Job, DeadLetter)>,
        permits: Arc<Semaphore>,
        hook_permits: Arc<Semaphore>,
        retries: Arc<Mutex<HashMap<u32, u32>>>,
        cancelled: Arc<AtomicUsize>,
        task: Option<JoinHandle<Result<(), FlatbedWorkerError>>>,
    }

    impl Harness {
        fn standalone(permits: usize) -> Self {
            let mut harness = Self::idle(permits);
            harness.start();
            harness
        }

        #[cfg(feature = "k8s")]
        fn ha(permits: usize, leading: bool) -> Self {
            let mut harness = Self::idle(permits);
            harness.ctx.leader = Some(tokio::sync::watch::channel(leading).0);
            harness.start();
            harness
        }

        fn idle(permits: usize) -> Self {
            let (started_tx, started) = mpsc::unbounded_channel();
            let (letters_tx, letters) = mpsc::unbounded_channel();
            let permits = Arc::new(Semaphore::new(permits));
            let hook_permits = Arc::new(Semaphore::new(Semaphore::MAX_PERMITS));
            let retries = Arc::default();
            let cancelled = Arc::default();
            Self {
                ctx: Ctx {
                    queues: LocalQueues::default(),
                    #[cfg(feature = "k8s")]
                    leader: None,
                },
                worker: Recorder {
                    started: started_tx,
                    letters: letters_tx,
                    permits: Arc::clone(&permits),
                    hook_permits: Arc::clone(&hook_permits),
                    retries: Arc::clone(&retries),
                    cancelled: Arc::clone(&cancelled),
                },
                started,
                letters,
                permits,
                hook_permits,
                retries,
                cancelled,
                task: None,
            }
        }

        /// Runs `serve` over this harness's context, as a worker (re)start
        /// does.
        fn start(&mut self) {
            let ctx = Arc::new(self.ctx.clone());
            self.task = Some(tokio::spawn(serve(self.worker.clone(), ctx)));
        }

        fn queue(&self) -> LocalQueueSender<Recorder> {
            self.ctx.local_queue::<Recorder>()
        }

        fn send(&self, key: u32, tag: &'static str) {
            self.queue().send(job(key, tag)).unwrap();
        }

        fn status(&self, key: u32) -> KeyStatus {
            self.queue().status(&key)
        }

        fn is_dead_lettered(&self, key: u32) -> bool {
            matches!(self.status(key), KeyStatus::DeadLettered(_))
        }

        async fn next_started(&mut self) -> Job {
            timeout(QUIET, self.started.recv())
                .await
                .expect("a message started")
                .unwrap()
        }

        async fn next_letter(&mut self) -> (Job, DeadLetter) {
            timeout(HANDLE_TIMEOUT + QUIET, self.letters.recv())
                .await
                .expect("a message was dead-lettered")
                .unwrap()
        }

        async fn assert_quiet(&mut self, within: Duration) {
            if let Ok(job) = timeout(within, self.started.recv()).await {
                panic!("nothing should start, but {job:?} did");
            }
        }
    }

    fn assert_letter(
        letter: &DeadLetter,
        cause: DeadLetterCause,
        attempts: u32,
        last_reason: Option<&str>,
    ) {
        assert_eq!(letter.cause, cause);
        assert_eq!(letter.attempts, attempts);
        assert_eq!(letter.last_reason.as_deref(), last_reason);
    }

    #[tokio::test(start_paused = true)]
    async fn producers_and_the_worker_share_one_queue_whoever_asks_first() {
        let mut h = Harness::idle(10);
        let before = h.queue();
        h.start();
        tokio::time::sleep(Duration::from_millis(1)).await;
        let after = h.ctx.clone().local_queue::<Recorder>();
        before.send(job(1, "before")).unwrap();
        after.send(job(2, "after")).unwrap();
        assert_eq!(h.next_started().await, job(1, "before"));
        assert_eq!(h.next_started().await, job(2, "after"));

        let mut h = Harness::standalone(10);
        tokio::time::sleep(Duration::from_millis(1)).await;
        h.send(3, "worker first");
        assert_eq!(h.next_started().await, job(3, "worker first"));
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

        assert_eq!(h.status(1), KeyStatus::Queued(1));
        assert_eq!(h.status(2), KeyStatus::Queued(2));
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
        assert_eq!(h.status(1), KeyStatus::InHand);
        h.permits.add_permits(1);
        assert_eq!(h.next_started().await, job(1, "second"));
    }

    #[tokio::test(start_paused = true)]
    async fn status_counts_the_line_from_the_next_message() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");
        assert_eq!(h.status(1), KeyStatus::Queued(1));
        assert_eq!(h.status(3), KeyStatus::Queued(3));
        assert_eq!(h.status(4), KeyStatus::Absent);

        h.next_started().await;
        assert_eq!(h.status(1), KeyStatus::InHand);
        assert_eq!(h.status(2), KeyStatus::Queued(1));
        assert_eq!(h.status(3), KeyStatus::Queued(2));

        h.permits.add_permits(1);
        h.next_started().await;
        assert_eq!(h.status(1), KeyStatus::Absent);
        assert_eq!(h.status(2), KeyStatus::InHand);
        assert_eq!(h.status(3), KeyStatus::Queued(1));
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
    async fn retrying_on_the_last_attempt_dead_letters_as_exhausted() {
        let mut h = Harness::standalone(10);
        h.retries.lock().unwrap().insert(1, 10);
        h.send(1, "a");
        h.send(2, "b");

        assert_eq!(h.next_started().await, job(1, "a"));
        assert_eq!(h.next_started().await, job(2, "b"));
        h.send(1, "resent");
        assert_eq!(h.next_started().await, job(1, "a"));
        assert_eq!(h.next_started().await, job(1, "a"));
        let (msg, letter) = h.next_letter().await;
        assert_eq!(msg, job(1, "a"));
        assert_letter(&letter, DeadLetterCause::Exhausted, 3, Some("again"));
        assert_eq!(h.status(1), KeyStatus::DeadLettered(letter));
        h.assert_quiet(QUIET).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_handle_outrunning_the_timeout_is_dropped_and_dead_lettered() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        assert_eq!(h.next_started().await, job(1, "a"));
        let started_at = Instant::now();

        let (msg, letter) = h.next_letter().await;
        assert_eq!(started_at.elapsed(), HANDLE_TIMEOUT);
        assert_eq!(msg, job(1, "a"));
        assert_letter(
            &letter,
            DeadLetterCause::TimedOut {
                after: HANDLE_TIMEOUT,
            },
            1,
            None,
        );
        assert_eq!(h.cancelled.load(Ordering::SeqCst), 1);
        h.permits.add_permits(1);
        assert_eq!(h.next_started().await, job(2, "b"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_err_dead_letters_the_message_as_failed_and_the_worker_keeps_going() {
        let mut h = Harness::standalone(10);
        h.send(1, "err");
        h.send(2, "b");

        assert_eq!(h.next_started().await, job(1, "err"));
        let (msg, letter) = h.next_letter().await;
        assert_eq!(msg, job(1, "err"));
        assert_letter(
            &letter,
            DeadLetterCause::Failed("boom".into()),
            1,
            Some("boom"),
        );
        assert_eq!(h.next_started().await, job(2, "b"));
        assert!(h.is_dead_lettered(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_panic_dead_letters_the_message_as_failed_and_the_worker_keeps_going() {
        let mut h = Harness::standalone(10);
        h.send(1, "panic");
        h.send(2, "b");

        assert_eq!(h.next_started().await, job(1, "panic"));
        let (_, letter) = h.next_letter().await;
        let failed = DeadLetterCause::Failed("panicked: kaboom".into());
        assert_letter(&letter, failed, 1, Some("panicked: kaboom"));
        assert_eq!(h.next_started().await, job(2, "b"));
        h.send(3, "c");
        assert_eq!(h.next_started().await, job(3, "c"));
        assert!(!h.task.as_ref().unwrap().is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn the_dead_letter_hook_holds_the_line_until_it_returns() {
        let mut h = Harness::standalone(10);
        h.hook_permits.forget_permits(Semaphore::MAX_PERMITS);
        h.send(1, "err");
        h.send(2, "b");

        assert_eq!(h.next_started().await, job(1, "err"));
        h.next_letter().await;
        assert!(h.is_dead_lettered(1));
        h.assert_quiet(QUIET).await;
        h.hook_permits.add_permits(1);
        assert_eq!(h.next_started().await, job(2, "b"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_dead_letter_hook_outrunning_the_timeout_lets_the_line_move_on() {
        let mut h = Harness::standalone(10);
        h.hook_permits.forget_permits(Semaphore::MAX_PERMITS);
        h.send(1, "err");
        h.send(2, "b");

        assert_eq!(h.next_started().await, job(1, "err"));
        h.next_letter().await;
        h.assert_quiet(HANDLE_TIMEOUT - Duration::from_millis(1))
            .await;
        assert_eq!(
            timeout(Duration::from_millis(2), h.started.recv()).await,
            Ok(Some(job(2, "b")))
        );
        assert!(h.is_dead_lettered(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_dead_lettered_key_refuses_resends_and_drops_a_queued_one() {
        let mut h = Harness::standalone(0);
        h.send(1, "err");
        assert_eq!(h.next_started().await, job(1, "err"));
        h.send(1, "resent while in hand");

        h.permits.add_permits(10);
        h.next_letter().await;
        assert!(h.is_dead_lettered(1));
        assert_eq!(
            h.queue().send(job(1, "again")),
            Err(SendError::DeadLettered)
        );
        h.assert_quiet(QUIET).await;
    }

    #[tokio::test(start_paused = true)]
    async fn requeue_puts_the_key_back_in_line_with_its_attempts_reset() {
        let mut h = Harness::standalone(10);
        h.retries.lock().unwrap().insert(1, 4);
        h.send(1, "a");
        for _ in 0..3 {
            assert_eq!(h.next_started().await, job(1, "a"));
        }
        let (_, letter) = h.next_letter().await;
        assert_letter(&letter, DeadLetterCause::Exhausted, 3, Some("again"));

        assert_eq!(h.queue().requeue(&1), Ok(()));
        assert_eq!(h.status(1), KeyStatus::Queued(1));
        assert_eq!(h.next_started().await, job(1, "a"));
        assert_eq!(h.next_started().await, job(1, "a"));
        h.assert_quiet(QUIET).await;
        assert_eq!(h.status(1), KeyStatus::Absent);
        assert!(h.letters.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn forget_drops_a_dead_letter_so_its_key_can_be_sent_again() {
        let mut h = Harness::standalone(10);
        h.send(1, "err");
        h.next_letter().await;

        assert!(h.queue().forget(&1));
        assert!(!h.queue().forget(&1));
        assert_eq!(h.status(1), KeyStatus::Absent);
        h.send(1, "b");
        h.next_started().await;
        assert_eq!(h.next_started().await, job(1, "b"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_oldest_dead_letter_is_evicted_past_capacity() {
        let mut h = Harness::standalone(10);
        h.send(1, "err");
        h.send(2, "err");
        h.send(3, "err");
        for _ in 0..3 {
            h.next_letter().await;
        }

        assert_eq!(h.status(1), KeyStatus::Absent);
        assert!(h.is_dead_lettered(2));
        assert!(h.is_dead_lettered(3));
        h.send(1, "a");
        for _ in 0..3 {
            h.next_started().await;
        }
        assert_eq!(h.next_started().await, job(1, "a"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_key_is_refused_when_full_and_coalescing_still_succeeds() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        h.send(3, "c");
        let full = SendError::Full(LocalQueueFull {
            worker: "recorder",
            capacity: 3,
        });
        assert_eq!(h.queue().send(job(4, "d")), Err(full));
        assert_eq!(h.queue().send(job(2, "again")), Ok(()));

        h.next_started().await;
        assert_eq!(h.queue().send(job(4, "d")), Err(full));

        h.permits.add_permits(1);
        h.next_started().await;
        assert_eq!(h.queue().send(job(4, "d")), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_restarted_worker_drops_the_interrupted_message_and_keeps_the_queue() {
        let mut h = Harness::standalone(0);
        h.send(1, "a");
        h.send(2, "b");
        assert_eq!(h.next_started().await, job(1, "a"));

        let first = h.task.take().unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(h.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(h.status(1), KeyStatus::InHand);

        h.start();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(h.status(1), KeyStatus::Absent);
        assert_eq!(h.status(2), KeyStatus::Queued(1));
        h.permits.add_permits(10);
        assert_eq!(h.next_started().await, job(2, "b"));
        h.assert_quiet(QUIET).await;
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn nothing_is_handled_while_not_leader() {
        let mut h = Harness::ha(10, false);
        h.send(1, "a");

        h.assert_quiet(QUIET).await;
        assert_eq!(h.status(1), KeyStatus::Queued(1));
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn the_first_message_waits_for_settle_after_becoming_leader() {
        let mut h = Harness::ha(10, false);
        h.send(1, "a");
        h.assert_quiet(QUIET).await;

        h.ctx.leader.as_ref().unwrap().send(true).unwrap();
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

        h.ctx.leader.as_ref().unwrap().send(false).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(h.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(h.status(1), KeyStatus::Absent);
        assert_eq!(h.status(2), KeyStatus::Absent);

        h.ctx.leader.as_ref().unwrap().send(true).unwrap();
        h.permits.add_permits(10);
        h.assert_quiet(QUIET).await;
        h.send(3, "c");
        assert_eq!(h.next_started().await, job(3, "c"));
    }

    #[cfg(feature = "k8s")]
    #[tokio::test(start_paused = true)]
    async fn losing_leadership_clears_the_dead_letters_and_the_attempt_counts() {
        let mut h = Harness::ha(2, true);
        h.retries.lock().unwrap().insert(2, 10);
        h.send(1, "err");
        h.send(2, "a");
        assert_eq!(h.next_started().await, job(1, "err"));
        h.next_letter().await;
        assert_eq!(h.next_started().await, job(2, "a"));
        assert_eq!(h.next_started().await, job(2, "a"));

        h.ctx.leader.as_ref().unwrap().send(false).unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(h.status(1), KeyStatus::Absent);
        assert_eq!(h.status(2), KeyStatus::Absent);

        h.ctx.leader.as_ref().unwrap().send(true).unwrap();
        h.send(1, "x");
        h.send(2, "a");
        h.permits.add_permits(10);
        assert_eq!(h.next_started().await, job(1, "x"));
        for _ in 0..3 {
            assert_eq!(h.next_started().await, job(2, "a"));
        }
        let (_, letter) = h.next_letter().await;
        assert_letter(&letter, DeadLetterCause::Exhausted, 3, Some("again"));
    }
}
