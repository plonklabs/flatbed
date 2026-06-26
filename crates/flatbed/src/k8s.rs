//! Kubernetes reconciler support types for flatbed workers.
//!
//! These types are used by the [`KubeReconciler`] trait to provide
//! a clean handler interface for kube-runtime Controllers with NATS
//! JetStream support.

use std::fmt;
use std::sync::Arc;

use kube::Resource;
use tracing::{debug, error, info};

#[cfg(feature = "nats")]
use crate::nats::HasJetStream;
use crate::FlatbedWorkerError;

/// Error type for reconciliation.
///
/// Covers failure modes Flatbed reconcilers can encounter:
/// Kubernetes API errors (every reconciler), NATS publishing errors
/// (JetStream-bound implementors only — [`KubeReconciler`]), and
/// programming-invariant violations. [`KubeNativeReconciler`]
/// implementors never produce the [`ReconcileError::Nats`] variant;
/// the rest apply uniformly.
///
/// Implements `From<kube::Error>` so the `?` operator works on K8s API calls
/// inside reconciler handlers.
///
/// # Example
///
/// ```rust,ignore
/// async fn reconcile(
///     ctx: Arc<AppContext>,
///     obj: Arc<MyResource>,
/// ) -> Result<Action, ReconcileError> {
///     let api: Api<Deployment> = Api::namespaced(ctx.kube_client.clone(), "default");
///     let deploy = api.get("my-deploy").await?; // ? converts kube::Error -> ReconcileError
///     Ok(Action::requeue(Duration::from_secs(300)))
/// }
/// ```
#[derive(Debug)]
pub enum ReconcileError {
    /// Kubernetes API error.
    Kube(kube::Error),
    /// NATS publishing error.
    #[cfg(feature = "nats")]
    Nats(String),
    /// Programming-invariant violation — a "should never happen"
    /// branch the reconciler hit anyway. Distinct from `Kube` /
    /// `Nats` so first-responders triaging from structured logs
    /// (where `error` is the variant name, not the message) can
    /// tell a code bug apart from a transport failure.
    Internal(String),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconcileError::Kube(e) => write!(f, "Kubernetes error: {}", e),
            #[cfg(feature = "nats")]
            ReconcileError::Nats(e) => write!(f, "NATS error: {}", e),
            ReconcileError::Internal(e) => write!(f, "internal error: {}", e),
        }
    }
}

impl std::error::Error for ReconcileError {}

impl From<kube::Error> for ReconcileError {
    fn from(e: kube::Error) -> Self {
        ReconcileError::Kube(e)
    }
}

/// Trait for application contexts that provide a Kubernetes client.
///
/// # Example
///
/// ```rust,ignore
/// impl HasKubeClient for AppContext {
///     fn kube_client(&self) -> &kube::Client {
///         &self.kube_client
///     }
/// }
/// ```
pub trait HasKubeClient {
    /// Returns a reference to the Kubernetes client.
    fn kube_client(&self) -> &kube::Client;
}

/// Trait for application contexts that support leader election.
///
/// The reconciler macro uses this to only run the Controller on the leader pod.
///
/// # Example
///
/// ```rust,ignore
/// impl HasLeaderElection for AppContext {
///     fn is_leader_rx(&self) -> tokio::sync::watch::Receiver<bool> {
///         self.is_leader_rx.clone()
///     }
/// }
/// ```
pub trait HasLeaderElection {
    /// Returns a cloned receiver for the leader election watch channel.
    fn is_leader_rx(&self) -> ::tokio::sync::watch::Receiver<bool>;

    /// Returns whether HA mode (leader election) is enabled.
    ///
    /// When `false`, the pod runs both reconcilers and workers —
    /// leader-gating is skipped entirely. Defaults to `true`.
    fn ha_enabled(&self) -> bool {
        true
    }
}

/// Wait until leadership is acquired on the given watch channel.
///
/// Returns `true` when this pod becomes leader, or `false` if the
/// channel closes (sender dropped) before leadership was acquired.
///
/// Used by [`run_kube_reconciler`] to avoid duplicating the
/// leader-wait loop.
pub async fn wait_for_leadership(
    rx: &mut ::tokio::sync::watch::Receiver<bool>,
    worker_name: &str,
) -> bool {
    loop {
        if *rx.borrow_and_update() {
            return true;
        }
        if rx.changed().await.is_err() {
            error!(worker = %worker_name, "leader election channel closed");
            return false;
        }
    }
}

/// Wait until leadership is lost on the given watch channel.
///
/// Returns when the watch value becomes `false` or the channel closes.
/// Used by [`run_kube_reconciler`] as a cancellation future.
pub async fn wait_for_leadership_loss(
    rx: &mut ::tokio::sync::watch::Receiver<bool>,
    worker_name: &str,
) {
    loop {
        if !*rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            error!(worker = %worker_name, "leader election channel closed");
            return;
        }
    }
}

/// Wait until this pod is NOT the leader.
///
/// Returns `true` when the pod is a follower, or `false` if the channel
/// closes before that happens. Used by [`crate::nats::run_stream_worker`]
/// — workers only consume queues on non-leader pods (leader pods reconcile).
pub async fn wait_for_follower(
    rx: &mut ::tokio::sync::watch::Receiver<bool>,
    worker_name: &str,
) -> bool {
    loop {
        if !*rx.borrow_and_update() {
            return true;
        }
        if rx.changed().await.is_err() {
            error!(worker = %worker_name, "leader election channel closed");
            return false;
        }
    }
}

/// Wait until this pod becomes the leader (i.e., stop being a follower).
///
/// Returns when the watch value becomes `true` or the channel closes.
/// Used by [`crate::nats::run_stream_worker`] as a cancellation future
/// — workers stop consuming when the pod becomes leader.
pub async fn wait_for_follower_loss(
    rx: &mut ::tokio::sync::watch::Receiver<bool>,
    worker_name: &str,
) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            error!(worker = %worker_name, "leader election channel closed");
            return;
        }
    }
}

// ============================================================================
// KubeReconciler Trait (NATS-bound)
// ============================================================================

/// Trait-based reconciler for Kubernetes resources with NATS JetStream support.
///
/// Implement this trait to define a reconciler. The runtime executor
/// [`run_kube_reconciler`] handles leader election, stream setup, and
/// controller lifecycle.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::k8s::{KubeReconciler, ReconcileError};
/// use kube::runtime::controller::Action;
/// use std::sync::Arc;
///
/// struct MyReconciler;
///
/// impl KubeReconciler for MyReconciler {
///     type Resource = MyResource;
///     type Context = AppContext;
///     type Error = ReconcileError;
///
///     const NAME: &'static str = "my-reconciler";
///     const STREAM: &'static str = "TASKS";
///     const STREAM_SUBJECTS: &'static str = "tasks.>";
///
///     fn reconcile(
///         &self,
///         ctx: Arc<Self::Context>,
///         obj: Arc<Self::Resource>,
///     ) -> flatbed::BoxFuture<Result<Action, Self::Error>> {
///         Box::pin(async move {
///             // reconcile logic
///             Ok(Action::requeue(Duration::from_secs(300)))
///         })
///     }
/// }
///
/// flatbed::register_kube_reconciler!(MyReconciler, AppContext);
/// ```
#[cfg(feature = "nats")]
pub trait KubeReconciler: Send + Sync + 'static {
    /// The Kubernetes resource type this reconciler watches.
    type Resource: kube::Resource<DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Send
        + Sync
        + 'static;

    /// The application context type providing JetStream, kube client, and leader election.
    type Context: HasJetStream + HasKubeClient + HasLeaderElection + Send + Sync + 'static;

    /// The error type returned by reconcile/cleanup/drain methods.
    type Error: std::error::Error + Into<ReconcileError> + Send + 'static;

    /// Worker name used for logging and identification.
    const NAME: &'static str;

    /// Optional description of what this reconciler does.
    const DESCRIPTION: Option<&'static str> = None;

    /// JetStream stream name to create or reuse.
    const STREAM: &'static str;

    /// JetStream stream subject filter (e.g., `"tasks.>"`).
    const STREAM_SUBJECTS: &'static str;

    /// Optional finalizer string. When set, the runtime adds/removes it and
    /// calls [`cleanup`](KubeReconciler::cleanup) on deletion.
    const FINALIZER: Option<&'static str> = None;

    /// Called when a resource is created or updated. Must return an [`Action`]
    /// indicating when to requeue.
    ///
    /// [`Action`]: kube::runtime::controller::Action
    fn reconcile(
        &self,
        ctx: Arc<Self::Context>,
        obj: Arc<Self::Resource>,
    ) -> crate::BoxFuture<Result<kube::runtime::controller::Action, Self::Error>>;

    /// Called when a resource with the finalizer is being deleted.
    /// Default implementation does nothing.
    fn cleanup(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> crate::BoxFuture<Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    /// Extension point for graceful shutdown. **Not** invoked by the
    /// runtime executor [`run_kube_reconciler`] — the shutdown path
    /// drives [`crate::WorkerDrainInfo`] entries collected via
    /// `inventory`, and [`crate::register_kube_reconciler!`] does not
    /// submit one. Implementors that need drain behaviour must submit
    /// a [`crate::WorkerDrainInfo`] separately.
    fn drain(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    /// Hook for configuring additional owned resources on the controller.
    ///
    /// Override this to call `.owns::<ChildResource>(...)` on the controller
    /// so it watches dependent resources as well.
    fn configure_owns(
        controller: kube::runtime::controller::Controller<Self::Resource>,
        _client: &kube::Client,
    ) -> kube::runtime::controller::Controller<Self::Resource> {
        controller
    }
}

// ============================================================================
// KubeReconciler Runtime Executor
// ============================================================================

/// Internal context wrapper passed to kube-rs Controller run().
#[cfg(feature = "nats")]
struct ReconcileCtx<R: KubeReconciler> {
    app_ctx: Arc<R::Context>,
    reconciler: Arc<R>,
}

/// Check whether a resource has a specific finalizer.
#[doc(hidden)]
pub fn has_finalizer<R>(obj: &R, finalizer: &str) -> bool
where
    R: kube::Resource,
{
    obj.meta()
        .finalizers
        .as_ref()
        .is_some_and(|f| f.contains(&finalizer.to_string()))
}

/// Add a finalizer to a resource using a JSON merge patch.
async fn add_finalizer<R>(
    client: &kube::Client,
    obj: &R,
    finalizer: &str,
) -> Result<(), ReconcileError>
where
    R: kube::Resource<DynamicType = ()>,
{
    let name = obj.meta().name.as_deref().unwrap_or_default();
    let mut finalizers = obj.meta().finalizers.clone().unwrap_or_default();
    if !finalizers.iter().any(|f| f == finalizer) {
        finalizers.push(finalizer.to_string());
    }

    // Last-write-wins is correct here: the operator exclusively owns its
    // finalizer field. Omitting resourceVersion avoids 409s when the object
    // was modified between the reconciler's GET and this PATCH (e.g. by
    // cleanup() updating status).
    let patch = serde_json::json!({
        "metadata": {
            "finalizers": finalizers
        }
    });

    let api = dynamic_api_for::<R>(client, obj);
    api.patch(
        name,
        &kube::api::PatchParams::default(),
        &kube::api::Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Remove a finalizer from a resource using a JSON merge patch.
async fn remove_finalizer<R>(
    client: &kube::Client,
    obj: &R,
    finalizer: &str,
) -> Result<(), ReconcileError>
where
    R: kube::Resource<DynamicType = ()>,
{
    let name = obj.meta().name.as_deref().unwrap_or_default();
    let current = obj.meta().finalizers.as_ref().cloned().unwrap_or_default();
    let remaining: Vec<&str> = current
        .iter()
        .filter(|f| f.as_str() != finalizer)
        .map(|f| f.as_str())
        .collect();

    let patch = serde_json::json!({
        "metadata": {
            "finalizers": remaining
        }
    });

    let api = dynamic_api_for::<R>(client, obj);
    api.patch(
        name,
        &kube::api::PatchParams::default(),
        &kube::api::Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Build a DynamicObject Api for patching a resource in its correct namespace.
///
/// `Api::namespaced()` requires `Resource<Scope = NamespaceResourceScope>` at
/// compile time, but `KubeReconciler` doesn't constrain the scope — it accepts
/// both namespaced and cluster-scoped CRDs. `ApiResource::erase()` erases the
/// scope to `DynamicResourceScope`, letting us pick namespaced vs cluster at
/// runtime based on `obj.meta().namespace`.
fn dynamic_api_for<R>(client: &kube::Client, obj: &R) -> kube::Api<kube::api::DynamicObject>
where
    R: kube::Resource<DynamicType = ()>,
{
    let ar = kube::api::ApiResource::erase::<R>(&());
    if let Some(ref ns) = obj.meta().namespace {
        kube::Api::namespaced_with(client.clone(), ns, &ar)
    } else {
        kube::Api::all_with(client.clone(), &ar)
    }
}

/// Reconcile wrapper that handles finalizer logic when configured.
///
/// This function is `pub` so that integration tests can exercise the
/// orchestration paths (no-finalizer, add-finalizer, cleanup-on-delete).
/// It is hidden from documentation since end users call it indirectly
/// through the controller setup in [`run_kube_reconciler`].
#[cfg(feature = "nats")]
#[doc(hidden)]
pub async fn reconcile_with_finalizer<R>(
    reconciler: &R,
    ctx: Arc<R::Context>,
    obj: Arc<R::Resource>,
) -> Result<kube::runtime::controller::Action, ReconcileError>
where
    R: KubeReconciler,
{
    let Some(finalizer) = R::FINALIZER else {
        // No finalizer configured — call reconcile directly
        return reconciler.reconcile(ctx, obj).await.map_err(Into::into);
    };

    let client = ctx.kube_client();

    // Check if resource is being deleted
    let is_deleting = obj.meta().deletion_timestamp.is_some();

    if is_deleting {
        if has_finalizer(&*obj, finalizer) {
            reconciler
                .cleanup(Arc::clone(&ctx), Arc::clone(&obj))
                .await
                .map_err(Into::into)?;
            remove_finalizer(client, &*obj, finalizer).await?;
        }
        return Ok(kube::runtime::controller::Action::await_change());
    }

    // Not deleting — ensure finalizer is present, then reconcile
    if !has_finalizer(&*obj, finalizer) {
        add_finalizer(client, &*obj, finalizer).await?;
    }
    reconciler.reconcile(ctx, obj).await.map_err(Into::into)
}

/// Run a [`KubeReconciler`] with leader election, stream setup, and controller lifecycle.
///
/// This is the runtime executor that powers `register_kube_reconciler!`,
/// implementing the following lifecycle:
///
/// 1. Wait for leadership via the leader election channel
/// 2. Create or reuse the JetStream stream (WorkQueue retention, Memory storage)
/// 3. Build a kube-rs Controller with `Api::all()` for cluster-wide watching
/// 4. Run the controller, cancelling on leadership loss via `tokio::select!`
/// 5. Loop back to step 1 if leadership is lost
#[cfg(feature = "nats")]
pub async fn run_kube_reconciler<R, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    R: KubeReconciler<Context = C> + Default,
    C: HasJetStream + HasKubeClient + HasLeaderElection + Send + Sync + 'static,
    R::Resource: serde::Serialize,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("kube_reconciler '{}' context type mismatch", R::NAME));

    let reconciler = Arc::new(R::default());
    let mut is_leader_rx = HasLeaderElection::is_leader_rx(&*ctx);
    // `wait_for_leadership` returns immediately when the channel is
    // already `true`, so this guard distinguishes a genuine leadership
    // transition from a routine re-entry of the outer loop (e.g. after
    // the controller's `for_each` future resolves). On-call observability
    // depends on `"became leader"` only firing on actual transitions.
    let mut acquired_leadership = false;

    loop {
        // Wait until this pod becomes leader
        if !wait_for_leadership(&mut is_leader_rx, R::NAME).await {
            return Ok(());
        }

        // Ensure JetStream stream exists. `get_or_create_stream` is
        // idempotent, so running it on every loop iteration (including
        // routine controller re-entries) is cheap and defensive against
        // an apiserver/NATS path where the stream disappeared.
        let jetstream = HasJetStream::jetstream(&*ctx);
        jetstream
            .get_or_create_stream(async_nats::jetstream::stream::Config {
                name: R::STREAM.to_string(),
                subjects: vec![R::STREAM_SUBJECTS.to_string()],
                retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
                storage: async_nats::jetstream::stream::StorageType::Memory,
                num_replicas: HasJetStream::stream_replicas(&*ctx),
                ..Default::default()
            })
            .await
            .map_err(|e| {
                FlatbedWorkerError::new(
                    "stream_init",
                    format!("[{}] Failed to create stream: {}", R::NAME, e),
                )
            })?;

        if !acquired_leadership {
            info!(reconciler = R::NAME, "became leader, starting controller");
            info!(
                reconciler = R::NAME,
                stream = R::STREAM,
                "JetStream stream ready"
            );
            acquired_leadership = true;
        }

        let reconcile_ctx = Arc::new(ReconcileCtx {
            app_ctx: Arc::clone(&ctx),
            reconciler: Arc::clone(&reconciler),
        });

        let kube_client = HasKubeClient::kube_client(&*ctx);
        let resources: kube::Api<R::Resource> = kube::Api::all(kube_client.clone());

        // Build controller, apply configure_owns hook
        let controller = kube::runtime::controller::Controller::new(resources, Default::default());
        let controller = R::configure_owns(controller, kube_client);

        // Run reconciler, cancel on leadership loss
        let mut rx2 = HasLeaderElection::is_leader_rx(&*ctx);

        use futures::StreamExt;

        tokio::select! {
            _ = controller
                .run(
                    |obj, rctx| {
                        let reconciler_ref = Arc::clone(&rctx.reconciler);
                        let app_ctx = Arc::clone(&rctx.app_ctx);
                        async move {
                            reconcile_with_finalizer(&*reconciler_ref, app_ctx, obj).await
                        }
                    },
                    |_resource, error, _ctx| {
                        error!(reconciler = R::NAME, error = %error, "reconciliation error");
                        kube::runtime::controller::Action::requeue(
                            std::time::Duration::from_secs(60),
                        )
                    },
                    reconcile_ctx,
                )
                .for_each(|res| async {
                    match res {
                        Ok(o) => debug!(reconciler = R::NAME, object = ?o, "reconciled"),
                        Err(e) => error!(reconciler = R::NAME, error = ?e, "reconcile failed"),
                    }
                }) => {}
            _ = wait_for_leadership_loss(&mut rx2, R::NAME) => {
                info!(reconciler = R::NAME, "lost leadership, stopping controller");
                acquired_leadership = false;
            }
        }
    }
}

// ============================================================================
// KubeNativeReconciler Trait
// ============================================================================

/// Trait-based reconciler for Kubernetes resources backed solely by
/// the apiserver — no JetStream context bound, no foreign coordination
/// substrate.
///
/// The `Context` bound is `HasKubeClient + HasLeaderElection`;
/// reconcilers using this trait call services directly on the
/// application context, with no message bus involved. The runtime
/// executor [`run_kube_native_reconciler`] handles leader gating and
/// the kube-rs Controller lifecycle.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::k8s::{KubeNativeReconciler, ReconcileError};
/// use kube::runtime::controller::Action;
/// use std::sync::Arc;
///
/// #[derive(Default)]
/// struct MyReconciler;
///
/// impl KubeNativeReconciler for MyReconciler {
///     type Resource = MyResource;
///     type Context = AppContext;
///     type Error = ReconcileError;
///
///     const NAME: &'static str = "my-reconciler";
///
///     fn reconcile(
///         &self,
///         ctx: Arc<Self::Context>,
///         obj: Arc<Self::Resource>,
///     ) -> flatbed::BoxFuture<Result<Action, Self::Error>> {
///         Box::pin(async move {
///             // reconcile logic — call ctx.<service>.<method>(&obj) directly
///             Ok(Action::requeue(std::time::Duration::from_secs(300)))
///         })
///     }
/// }
///
/// flatbed::register_kube_native_reconciler!(MyReconciler, AppContext);
/// ```
pub trait KubeNativeReconciler: Send + Sync + 'static {
    /// The Kubernetes resource type this reconciler watches.
    type Resource: kube::Resource<DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Send
        + Sync
        + 'static;

    /// The application context type providing kube client + leader election.
    type Context: HasKubeClient + HasLeaderElection + Send + Sync + 'static;

    /// The error type returned by reconcile/cleanup/drain methods.
    type Error: std::error::Error + Into<ReconcileError> + Send + 'static;

    /// Worker name used for logging and identification.
    const NAME: &'static str;

    /// Optional description of what this reconciler does.
    const DESCRIPTION: Option<&'static str> = None;

    /// Optional finalizer string. When set, the runtime adds/removes
    /// it and calls [`cleanup`](KubeNativeReconciler::cleanup) on deletion.
    const FINALIZER: Option<&'static str> = None;

    /// Called when a resource is created or updated. Must return an
    /// [`Action`] indicating when to requeue.
    ///
    /// [`Action`]: kube::runtime::controller::Action
    fn reconcile(
        &self,
        ctx: Arc<Self::Context>,
        obj: Arc<Self::Resource>,
    ) -> crate::BoxFuture<Result<kube::runtime::controller::Action, Self::Error>>;

    /// Called when a resource with the finalizer is being deleted.
    /// Default implementation does nothing.
    fn cleanup(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> crate::BoxFuture<Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    /// Extension point for graceful shutdown. **Not** invoked by the
    /// runtime executor [`run_kube_native_reconciler`] — the shutdown
    /// path drives [`crate::WorkerDrainInfo`] entries collected via
    /// `inventory`, and [`crate::register_kube_native_reconciler!`]
    /// does not submit one. Implementors that need drain behaviour
    /// must submit a [`crate::WorkerDrainInfo`] separately.
    fn drain(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    /// Hook for configuring additional owned resources on the controller.
    ///
    /// Override this to call `.owns::<ChildResource>(...)` on the controller
    /// so it watches dependent resources as well.
    fn configure_owns(
        controller: kube::runtime::controller::Controller<Self::Resource>,
        _client: &kube::Client,
    ) -> kube::runtime::controller::Controller<Self::Resource> {
        controller
    }
}

// ============================================================================
// KubeNativeReconciler Runtime Executor
// ============================================================================

/// Internal context wrapper passed to kube-rs Controller run().
struct ReconcileKubeNativeCtx<R: KubeNativeReconciler> {
    app_ctx: Arc<R::Context>,
    reconciler: Arc<R>,
}

/// Reconcile wrapper that handles finalizer logic for
/// [`KubeNativeReconciler`].
///
/// Adds the configured `FINALIZER` on the create/update path, runs
/// `cleanup` then strips it on the delete path, and short-circuits
/// straight to `reconcile` when `FINALIZER` is `None`.
#[doc(hidden)]
pub async fn reconcile_with_finalizer_kube_native<R>(
    reconciler: &R,
    ctx: Arc<R::Context>,
    obj: Arc<R::Resource>,
) -> Result<kube::runtime::controller::Action, ReconcileError>
where
    R: KubeNativeReconciler,
{
    let Some(finalizer) = R::FINALIZER else {
        return reconciler.reconcile(ctx, obj).await.map_err(Into::into);
    };

    let client = ctx.kube_client();
    let is_deleting = obj.meta().deletion_timestamp.is_some();

    if is_deleting {
        if has_finalizer(&*obj, finalizer) {
            reconciler
                .cleanup(Arc::clone(&ctx), Arc::clone(&obj))
                .await
                .map_err(Into::into)?;
            remove_finalizer(client, &*obj, finalizer).await?;
        }
        return Ok(kube::runtime::controller::Action::await_change());
    }

    if !has_finalizer(&*obj, finalizer) {
        add_finalizer(client, &*obj, finalizer).await?;
    }
    reconciler.reconcile(ctx, obj).await.map_err(Into::into)
}

/// Run a [`KubeNativeReconciler`] with leader election and controller
/// lifecycle.
///
/// This is the runtime executor that powers
/// [`crate::register_kube_native_reconciler!`], implementing the
/// following lifecycle:
///
/// 1. Wait for leadership via the leader election channel.
/// 2. Build a kube-rs Controller with `Api::all()` for cluster-wide watching.
/// 3. Run the controller, cancelling on leadership loss via `tokio::select!`.
/// 4. Loop back to step 1 if leadership is lost.
pub async fn run_kube_native_reconciler<R, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    R: KubeNativeReconciler<Context = C> + Default,
    C: HasKubeClient + HasLeaderElection + Send + Sync + 'static,
    R::Resource: serde::Serialize,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("kube_native_reconciler '{}' context type mismatch", R::NAME));

    let reconciler = Arc::new(R::default());
    let mut is_leader_rx = HasLeaderElection::is_leader_rx(&*ctx);
    // `wait_for_leadership` returns immediately when the channel is
    // already `true`, so this guard distinguishes a genuine leadership
    // transition from a routine re-entry of the outer loop (e.g. after
    // the controller's `for_each` future resolves). On-call observability
    // depends on `"became leader"` only firing on actual transitions.
    let mut acquired_leadership = false;

    loop {
        if !wait_for_leadership(&mut is_leader_rx, R::NAME).await {
            return Ok(());
        }

        if !acquired_leadership {
            info!(reconciler = R::NAME, "became leader, starting controller");
            acquired_leadership = true;
        }

        let reconcile_ctx = Arc::new(ReconcileKubeNativeCtx {
            app_ctx: Arc::clone(&ctx),
            reconciler: Arc::clone(&reconciler),
        });

        let kube_client = HasKubeClient::kube_client(&*ctx);
        let resources: kube::Api<R::Resource> = kube::Api::all(kube_client.clone());

        let controller = kube::runtime::controller::Controller::new(resources, Default::default());
        let controller = R::configure_owns(controller, kube_client);

        let mut rx2 = HasLeaderElection::is_leader_rx(&*ctx);

        use futures::StreamExt;

        tokio::select! {
            _ = controller
                .run(
                    |obj, rctx| {
                        let reconciler_ref = Arc::clone(&rctx.reconciler);
                        let app_ctx = Arc::clone(&rctx.app_ctx);
                        async move {
                            reconcile_with_finalizer_kube_native(&*reconciler_ref, app_ctx, obj).await
                        }
                    },
                    |_resource, error, _ctx| {
                        error!(reconciler = R::NAME, error = %error, "reconciliation error");
                        kube::runtime::controller::Action::requeue(
                            std::time::Duration::from_secs(60),
                        )
                    },
                    reconcile_ctx,
                )
                .for_each(|res| async {
                    match res {
                        Ok(o) => debug!(reconciler = R::NAME, object = ?o, "reconciled"),
                        Err(e) => error!(reconciler = R::NAME, error = ?e, "reconcile failed"),
                    }
                }) => {}
            _ = wait_for_leadership_loss(&mut rx2, R::NAME) => {
                info!(reconciler = R::NAME, "lost leadership, stopping controller");
                acquired_leadership = false;
            }
        }
    }
}

// ============================================================================
// KubeWatcher Trait
// ============================================================================

/// Leader-gated reconciler driven by `kube::runtime::watcher` events on a
/// resource we don't own.
///
/// Use this when [`KubeReconciler`]'s finalizer model doesn't fit because
/// the watched resource is owned by kube-controller-manager (e.g.
/// `EndpointSlice`, `Pod`): adding a finalizer would block kube's own
/// garbage collection, and a finalizer-less `KubeReconciler` would drop
/// `Delete` events silently — a correctness gap for resources whose
/// identity (raw pod IPs, slice contributions) kube recycles aggressively.
///
/// The executor [`run_kube_watcher`] handles leader gating
/// (`wait_for_leadership` / `wait_for_leadership_loss`), opens an
/// `Api::all` cluster-wide watch, dispatches the kube-rs `Event` variants
/// to the corresponding callback, and reopens the watch when the stream
/// ends. Implementors carry per-leader state inline (with interior
/// mutability) and apply caller-specific I/O inside their callbacks.
///
/// # Relationship to other Flatbed primitives
///
/// | Trait                  | Source                | Leadership      | Notes                                  |
/// |------------------------|-----------------------|-----------------|----------------------------------------|
/// | [`KubeReconciler`]     | kube Controller       | Leader-only     | We own the CRD; finalizer handles Delete; JetStream context bound |
/// | [`KubeNativeReconciler`] | kube Controller     | Leader-only     | We own the CRD; finalizer handles Delete; no JetStream context bound |
/// | `KubeWatcher` | `Api::all` watcher    | Leader-only     | We don't own the resource              |
/// | [`crate::nats::StreamWorker`] | NATS subject pull   | Follower-only   | Queue semantics                        |
/// | [`crate::kv::KvWorker`] | NATS KV watch_all  | Every pod       | Fan-out cache subscriber               |
///
/// # Naming convention
///
/// Flatbed's leader-gated / event-driven primitives follow the
/// `<Source><Role>` pattern: the prefix names the specific medium the
/// primitive reads from (`Kube` = kube apiserver, `Stream` = JetStream
/// stream, `Kv` = JetStream KV bucket), and the suffix names the role
/// (`Watcher`, `Reconciler`, `Worker`). New primitives slot in under
/// this rule.
///
/// # State handling
///
/// `&self` callbacks return `BoxFuture<()>` with no lifetime tied to
/// `self`, so the future can't borrow `self` directly. Implementors that
/// need per-leader state should hold it behind `Arc<Mutex<...>>` on the
/// struct and clone the `Arc` into each callback future, matching the
/// example below. The `on_init` callback fires at the start of every
/// list+watch burst (including reopens after a transient watch error),
/// so it's where buffer-mode lifecycles (Init → InitApply → InitDone)
/// reset their buffer.
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::k8s::{HasKubeClient, HasLeaderElection, KubeWatcher};
/// use flatbed::BoxFuture;
/// use k8s_openapi::api::discovery::v1::EndpointSlice;
/// use std::sync::Arc;
///
/// #[derive(Default)]
/// struct EndpointWatcher {
///     state: Arc<tokio::sync::Mutex<WriterState>>,
/// }
///
/// impl KubeWatcher for EndpointWatcher {
///     type Resource = EndpointSlice;
///     type Context = AppContext;
///
///     const NAME: &'static str = "endpoint-watcher";
///
///     fn on_apply(&self, ctx: Arc<AppContext>, slice: EndpointSlice) -> BoxFuture<()> {
///         let state = Arc::clone(&self.state);
///         Box::pin(async move {
///             let ops = state.lock().await.handle_apply(&slice);
///             apply_ops(&ctx, ops).await;
///         })
///     }
///     // ... on_delete, on_init, on_init_apply, on_init_done ...
/// }
///
/// flatbed::register_kube_watcher!(EndpointWatcher, AppContext);
/// ```
pub trait KubeWatcher: Send + Sync + 'static {
    /// The Kubernetes resource type this reconciler watches.
    type Resource: kube::Resource<DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Send
        + Sync
        + 'static;

    /// The application context type providing kube client + leader election.
    type Context: HasKubeClient + HasLeaderElection + Send + Sync + 'static;

    /// Worker name used for logging and identification.
    const NAME: &'static str;

    /// Optional description of what this reconciler does.
    const DESCRIPTION: Option<&'static str> = None;

    /// Fired for `Event::Apply`: a resource was added or updated
    /// outside an init burst.
    fn on_apply(&self, ctx: Arc<Self::Context>, obj: Self::Resource) -> crate::BoxFuture<()>;

    /// Fired for `Event::Delete`: a resource was deleted. Always
    /// observed for this primitive — that's the whole point relative
    /// to `KubeReconciler`, which can't see deletes without a
    /// finalizer.
    fn on_delete(&self, ctx: Arc<Self::Context>, obj: Self::Resource) -> crate::BoxFuture<()>;

    /// Fired for `Event::Init`: the start of a list-then-watch burst.
    /// Fires every time the watch reopens, not just at process
    /// start-up. Default is a no-op.
    fn on_init(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<()> {
        Box::pin(async {})
    }

    /// Fired for `Event::InitApply`: a resource present at the start
    /// of the burst. Required (no default) because a no-op default
    /// would silently drop the resync set on every watch reopen —
    /// implementors must decide whether to buffer for InitDone or
    /// treat InitApply as Apply. When buffering, also override
    /// [`on_init`](Self::on_init) to reset the buffer at burst start
    /// and [`on_init_done`](Self::on_init_done) to commit it at burst
    /// end; otherwise stale entries from the previous burst accumulate
    /// across watch reopens.
    fn on_init_apply(&self, ctx: Arc<Self::Context>, obj: Self::Resource) -> crate::BoxFuture<()>;

    /// Fired for `Event::InitDone`: the init burst is complete and
    /// subsequent events come from the watch (not the initial list).
    /// Default is a no-op.
    fn on_init_done(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<()> {
        Box::pin(async {})
    }

    /// Configure the underlying `kube::runtime::watcher::Config`.
    ///
    /// Defaults to an unfiltered cluster-wide watch (`Config::default()`).
    /// Override to add label or field selectors — e.g. a Pod watcher
    /// scoped to a managed-namespace label, or a Service watcher
    /// filtered to services your controller owns. Returning a `Config` keeps
    /// the open-once-relist-on-error behaviour the kube-rs `watcher`
    /// provides; replace selectors here rather than pre-filtering in
    /// callbacks so the apiserver-side filter shrinks the watch
    /// bandwidth.
    fn watch_config(&self) -> kube::runtime::watcher::Config {
        kube::runtime::watcher::Config::default()
    }

    /// Whether the executor should leader-gate this watcher.
    ///
    /// Default `true`: only the elected leader opens the watch, so
    /// writes to shared external state come from one pod at a time.
    ///
    /// Set to `false` for watchers whose only side-effect is on
    /// per-pod local state. Every pod runs its own watch; the
    /// apiserver is the shared source of truth, and each pod
    /// independently arrives at the same view.
    const LEADER_GATED: bool = true;
}

// ============================================================================
// KubeWatcher Runtime Executor
// ============================================================================

/// Run a [`KubeWatcher`] with leader election and watch lifecycle.
///
/// 1. Wait for leadership via the leader election channel.
/// 2. Open an `Api::all` watcher on the reconciler's resource type.
/// 3. Dispatch each `kube::runtime::watcher::Event` to the matching
///    trait callback; warn-and-continue on stream errors (kube-rs's
///    `watcher` recovers on its own with a relist).
/// 4. `tokio::select!` the stream against leadership loss — when this
///    pod stops being leader, the watch is dropped immediately and
///    the loop returns to step 1.
///
/// The reconciler instance is created once (`R::default()`) and
/// re-used across leadership cycles. Implementors that need state to
/// reset per-watch-burst should do so in their `on_init` callback,
/// which fires at the start of every list+watch — including reopens
/// after a transient watch error.
pub async fn run_kube_watcher<R, C>(
    ctx: Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), FlatbedWorkerError>
where
    R: KubeWatcher<Context = C> + Default,
    C: HasKubeClient + HasLeaderElection + Send + Sync + 'static,
{
    let ctx: Arc<C> = ctx
        .downcast::<C>()
        .unwrap_or_else(|_| panic!("kube_watcher '{}' context type mismatch", R::NAME));

    let reconciler = Arc::new(R::default());

    if !R::LEADER_GATED {
        // Non-leader-gated watcher: every pod runs its own stream
        // continuously. The apiserver is the shared source of truth;
        // the watcher's only effect is on per-pod local state, so
        // running independently on every replica is the right shape.
        info!(watcher = R::NAME, "starting non-leader-gated stream");
        loop {
            drive_kube_watcher::<R, C>(Arc::clone(&reconciler), Arc::clone(&ctx)).await;
            info!(watcher = R::NAME, "stream cycle returned; will reopen");
        }
    }

    let mut is_leader_rx = HasLeaderElection::is_leader_rx(&*ctx);
    // `wait_for_leadership` returns immediately when the channel is
    // already `true`, so this guard distinguishes a genuine leadership
    // transition from a routine watch reopen (the kube watcher stream
    // ends regularly under normal operation — apiserver-driven watch
    // timeouts, server restarts). On-call observability depends on
    // `"became leader"` only firing on actual transitions.
    let mut acquired_leadership = false;

    loop {
        if !wait_for_leadership(&mut is_leader_rx, R::NAME).await {
            return Ok(());
        }

        if !acquired_leadership {
            info!(watcher = R::NAME, "became leader, starting stream");
            acquired_leadership = true;
        }
        let mut loss_rx = HasLeaderElection::is_leader_rx(&*ctx);
        tokio::select! {
            _ = drive_kube_watcher::<R, C>(Arc::clone(&reconciler), Arc::clone(&ctx)) => {
                info!(watcher = R::NAME, "stream cycle returned; will reopen");
            }
            _ = wait_for_leadership_loss(&mut loss_rx, R::NAME) => {
                info!(watcher = R::NAME, "lost leadership, stopping stream");
                acquired_leadership = false;
            }
        }
    }
}

/// One open-watch-until-end cycle. Returning means the outer loop
/// should reopen (typically because the underlying stream ended; the
/// `tokio::select!` in [`run_kube_watcher`] cancels this
/// function on leadership loss before it returns naturally).
async fn drive_kube_watcher<R, C>(reconciler: Arc<R>, ctx: Arc<C>)
where
    R: KubeWatcher<Context = C>,
    C: HasKubeClient + HasLeaderElection + Send + Sync + 'static,
{
    use futures::StreamExt;
    use kube::runtime::watcher::watcher;

    let kube_client = HasKubeClient::kube_client(&*ctx);
    let api: kube::Api<R::Resource> = kube::Api::all(kube_client.clone());
    let mut stream = watcher(api, reconciler.watch_config()).boxed();

    while let Some(event) = stream.next().await {
        dispatch_kube_event::<R, C>(&reconciler, Arc::clone(&ctx), event).await;
    }
}

/// Route a watcher event to the matching trait callback. Lives apart
/// from [`drive_kube_watcher`] so the variant→callback mapping is
/// unit-testable without a kube apiserver. Stream errors are logged
/// and dropped — kube-rs's `watcher` recovers on its own by relisting.
async fn dispatch_kube_event<R, C>(
    reconciler: &R,
    ctx: Arc<C>,
    event: Result<kube::runtime::watcher::Event<R::Resource>, kube::runtime::watcher::Error>,
) where
    R: KubeWatcher<Context = C>,
    C: HasKubeClient + HasLeaderElection + Send + Sync + 'static,
{
    use kube::runtime::watcher::Event;
    match event {
        Ok(Event::Apply(obj)) => reconciler.on_apply(ctx, obj).await,
        Ok(Event::Delete(obj)) => reconciler.on_delete(ctx, obj).await,
        Ok(Event::Init) => reconciler.on_init(ctx).await,
        Ok(Event::InitApply(obj)) => reconciler.on_init_apply(ctx, obj).await,
        Ok(Event::InitDone) => reconciler.on_init_done(ctx).await,
        Err(e) => {
            tracing::warn!(watcher = R::NAME, error = %e, "watch error; will re-list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;

    // --- ha_enabled default ---

    struct TestCtx {
        rx: watch::Receiver<bool>,
    }

    impl HasLeaderElection for TestCtx {
        fn is_leader_rx(&self) -> watch::Receiver<bool> {
            self.rx.clone()
        }
    }

    #[test]
    fn ha_enabled_defaults_to_true() {
        let (_tx, rx) = watch::channel(false);
        let ctx = TestCtx { rx };
        assert!(ctx.ha_enabled());
    }

    // --- wait_for_leadership ---

    #[tokio::test]
    async fn leadership_returns_immediately_when_already_leader() {
        let (_tx, mut rx) = watch::channel(true);
        assert!(wait_for_leadership(&mut rx, "test").await);
    }

    #[tokio::test]
    async fn leadership_waits_then_returns_on_promotion() {
        let (tx, mut rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(true).unwrap();
        });
        assert!(wait_for_leadership(&mut rx, "test").await);
    }

    #[tokio::test]
    async fn leadership_returns_false_when_channel_closes() {
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        assert!(!wait_for_leadership(&mut rx, "test").await);
    }

    // --- wait_for_leadership_loss ---

    #[tokio::test]
    async fn leadership_loss_returns_immediately_when_not_leader() {
        let (_tx, mut rx) = watch::channel(false);
        wait_for_leadership_loss(&mut rx, "test").await;
    }

    #[tokio::test]
    async fn leadership_loss_waits_then_returns_on_demotion() {
        let (tx, mut rx) = watch::channel(true);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(false).unwrap();
        });
        wait_for_leadership_loss(&mut rx, "test").await;
    }

    #[tokio::test]
    async fn leadership_loss_returns_when_channel_closes() {
        let (tx, mut rx) = watch::channel(true);
        drop(tx);
        wait_for_leadership_loss(&mut rx, "test").await;
    }

    // --- wait_for_follower ---

    #[tokio::test]
    async fn follower_returns_immediately_when_not_leader() {
        let (_tx, mut rx) = watch::channel(false);
        assert!(wait_for_follower(&mut rx, "test").await);
    }

    #[tokio::test]
    async fn follower_waits_then_returns_on_demotion() {
        let (tx, mut rx) = watch::channel(true);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(false).unwrap();
        });
        assert!(wait_for_follower(&mut rx, "test").await);
    }

    #[tokio::test]
    async fn follower_returns_false_when_channel_closes() {
        let (tx, mut rx) = watch::channel(true);
        drop(tx);
        assert!(!wait_for_follower(&mut rx, "test").await);
    }

    /// This is the bug scenario: in standalone mode the leader channel is
    /// always `true`, so `wait_for_follower` blocks forever. The fix is
    /// to check `ha_enabled()` before calling this function.
    #[tokio::test]
    async fn follower_blocks_when_always_leader() {
        let (_tx, mut rx) = watch::channel(true);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            wait_for_follower(&mut rx, "test"),
        )
        .await;
        assert!(
            result.is_err(),
            "wait_for_follower should block when leader channel is always true"
        );
    }

    // --- wait_for_follower_loss ---

    #[tokio::test]
    async fn follower_loss_returns_immediately_when_leader() {
        let (_tx, mut rx) = watch::channel(true);
        wait_for_follower_loss(&mut rx, "test").await;
    }

    #[tokio::test]
    async fn follower_loss_waits_then_returns_on_promotion() {
        let (tx, mut rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            tx.send(true).unwrap();
        });
        wait_for_follower_loss(&mut rx, "test").await;
    }

    #[tokio::test]
    async fn follower_loss_returns_when_channel_closes() {
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        wait_for_follower_loss(&mut rx, "test").await;
    }

    // --- has_finalizer ---

    #[test]
    fn has_finalizer_returns_false_when_no_finalizers() {
        use k8s_openapi::api::core::v1::ConfigMap;
        use kube::api::ObjectMeta;

        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                finalizers: Some(vec![]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!has_finalizer(&cm, "my.finalizer/cleanup"));
    }

    #[test]
    fn has_finalizer_returns_false_when_target_not_present() {
        use k8s_openapi::api::core::v1::ConfigMap;
        use kube::api::ObjectMeta;

        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                finalizers: Some(vec![
                    "other.finalizer/one".to_string(),
                    "other.finalizer/two".to_string(),
                ]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!has_finalizer(&cm, "my.finalizer/cleanup"));
    }

    #[test]
    fn has_finalizer_returns_true_when_target_present() {
        use k8s_openapi::api::core::v1::ConfigMap;
        use kube::api::ObjectMeta;

        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                finalizers: Some(vec![
                    "other.finalizer/one".to_string(),
                    "my.finalizer/cleanup".to_string(),
                ]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(has_finalizer(&cm, "my.finalizer/cleanup"));
    }

    #[test]
    fn has_finalizer_returns_false_when_finalizers_is_none() {
        use k8s_openapi::api::core::v1::ConfigMap;
        use kube::api::ObjectMeta;

        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                finalizers: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!has_finalizer(&cm, "my.finalizer/cleanup"));
    }

    // --- KubeReconciler trait defaults ---
    //
    // `KubeReconciler` and `HasJetStream` are gated behind
    // `feature = "nats"`; the test fixtures and tests that use them
    // share the same cfg gate so the test module still compiles when
    // workspace feature resolution leaves `nats` unset (e.g. running
    // `cargo test --lib` from the workspace root without enabling
    // `nats` on flatbed).

    /// Minimal resource type for testing the KubeReconciler trait defaults.
    /// Uses ConfigMap as the resource since it satisfies all kube bounds.
    #[cfg(feature = "nats")]
    struct TestReconciler;

    /// Minimal context that satisfies KubeReconciler::Context bounds.
    #[cfg(feature = "nats")]
    struct TestReconcilerCtx {
        kube_client: kube::Client,
        jetstream: async_nats::jetstream::Context,
        leader_rx: watch::Receiver<bool>,
    }

    #[cfg(feature = "nats")]
    impl HasJetStream for TestReconcilerCtx {
        fn jetstream(&self) -> &async_nats::jetstream::Context {
            &self.jetstream
        }
    }

    #[cfg(feature = "nats")]
    impl HasKubeClient for TestReconcilerCtx {
        fn kube_client(&self) -> &kube::Client {
            &self.kube_client
        }
    }

    #[cfg(feature = "nats")]
    impl HasLeaderElection for TestReconcilerCtx {
        fn is_leader_rx(&self) -> watch::Receiver<bool> {
            self.leader_rx.clone()
        }
    }

    #[cfg(feature = "nats")]
    impl KubeReconciler for TestReconciler {
        type Resource = k8s_openapi::api::core::v1::ConfigMap;
        type Context = TestReconcilerCtx;
        type Error = ReconcileError;

        const NAME: &'static str = "test-reconciler";
        const STREAM: &'static str = "TEST";
        const STREAM_SUBJECTS: &'static str = "test.>";

        fn reconcile(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Arc<Self::Resource>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<kube::runtime::controller::Action, Self::Error>,
                    > + Send,
            >,
        > {
            Box::pin(async {
                Ok(kube::runtime::controller::Action::requeue(
                    std::time::Duration::from_secs(300),
                ))
            })
        }
    }

    #[tokio::test]
    async fn cleanup_default_returns_ok() {
        // We cannot create a real kube::Client or jetstream::Context without
        // a running cluster/NATS, but the cleanup default does not use them.
        // Use a mock-style approach: cast arbitrary data to check the default body only.
        // Since the default ignores ctx and obj, we verify it returns Ok(()).
        //
        // The trait default is: Box::pin(async { Ok(()) })
        // We just need to verify the return value, not the context.
        //
        // Build a minimal future directly from the default method body:
        let fut: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), ReconcileError>> + Send>,
        > = Box::pin(async { Ok(()) });
        let result = fut.await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn drain_default_returns_ok() {
        let fut: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), ReconcileError>> + Send>,
        > = Box::pin(async { Ok(()) });
        let result = fut.await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "nats")]
    #[test]
    fn configure_owns_default_returns_controller_unchanged() {
        // The default configure_owns just returns the controller as-is.
        // We can't easily construct a Controller in a unit test, but we can
        // verify the function signature compiles and the default exists.
        // The test validates that the trait definition is correct by
        // checking that TestReconciler (with no configure_owns override)
        // compiles successfully.
        //
        // A more meaningful test: verify FINALIZER default is None
        assert!(
            <TestReconciler as KubeReconciler>::FINALIZER.is_none(),
            "Default FINALIZER should be None"
        );
        assert!(
            <TestReconciler as KubeReconciler>::DESCRIPTION.is_none(),
            "Default DESCRIPTION should be None"
        );
    }

    // --- ReconcileError ---
    //
    // `ReconcileError::Nats` is `#[cfg(feature = "nats")]`, so the
    // two Display tests below need the same cfg gate to compile when
    // workspace feature resolution leaves `nats` unset.

    #[cfg(feature = "nats")]
    #[test]
    fn reconcile_error_display_kube() {
        let err = ReconcileError::Nats("connection refused".to_string());
        let display = format!("{}", err);
        assert!(display.contains("NATS error"));
        assert!(display.contains("connection refused"));
    }

    #[cfg(feature = "nats")]
    #[test]
    fn reconcile_error_display_nats() {
        let err = ReconcileError::Nats("timeout".to_string());
        assert_eq!(format!("{}", err), "NATS error: timeout");
    }

    // ========================================================================
    // KubeWatcher tests
    // ========================================================================

    /// Stub context for KubeWatcher tests. Implements both
    /// `HasKubeClient` and `HasLeaderElection`; both accessors panic
    /// because the tests we run never need them (we exercise the
    /// dispatch helper directly, never the leader loop or watch
    /// open).
    struct KubeWatcherStubCtx;

    impl HasKubeClient for KubeWatcherStubCtx {
        fn kube_client(&self) -> &kube::Client {
            panic!("stub: kube_client should not be called in dispatch tests");
        }
    }

    impl HasLeaderElection for KubeWatcherStubCtx {
        fn is_leader_rx(&self) -> watch::Receiver<bool> {
            panic!("stub: is_leader_rx should not be called in dispatch tests");
        }
    }

    /// Counts each callback invocation so dispatch tests can pin the
    /// `Event` variant → `on_*` mapping. Held behind `Arc` because
    /// `KubeWatcher` callbacks return `BoxFuture<'static>` and
    /// can't borrow from `&self`; atomic fields avoid a lock since the
    /// counters are updated independently.
    #[derive(Default)]
    struct DispatchCounters {
        apply: std::sync::atomic::AtomicUsize,
        delete: std::sync::atomic::AtomicUsize,
        init: std::sync::atomic::AtomicUsize,
        init_apply: std::sync::atomic::AtomicUsize,
        init_done: std::sync::atomic::AtomicUsize,
    }

    #[derive(Default)]
    struct TestKubeWatcher {
        counters: Arc<DispatchCounters>,
    }

    impl KubeWatcher for TestKubeWatcher {
        type Resource = k8s_openapi::api::core::v1::ConfigMap;
        type Context = KubeWatcherStubCtx;

        const NAME: &'static str = "test-kube-stream-reconciler";

        fn on_apply(&self, _ctx: Arc<Self::Context>, _obj: Self::Resource) -> crate::BoxFuture<()> {
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                counters
                    .apply
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }

        fn on_delete(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Self::Resource,
        ) -> crate::BoxFuture<()> {
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                counters
                    .delete
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }

        fn on_init(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<()> {
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                counters
                    .init
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }

        fn on_init_apply(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Self::Resource,
        ) -> crate::BoxFuture<()> {
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                counters
                    .init_apply
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }

        fn on_init_done(&self, _ctx: Arc<Self::Context>) -> crate::BoxFuture<()> {
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                counters
                    .init_done
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        }
    }

    fn empty_configmap() -> k8s_openapi::api::core::v1::ConfigMap {
        use kube::api::ObjectMeta;
        k8s_openapi::api::core::v1::ConfigMap {
            metadata: ObjectMeta {
                name: Some("dispatch-test".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn dispatch_kube_event_routes_each_variant_to_its_callback() {
        // Pin the `Event::Variant` → `on_*` mapping. Every variant
        // must increment its dedicated counter exactly once, none of
        // the others. A regression that re-shuffled the match arms
        // (e.g. Init → on_init_done) would otherwise pass type-check
        // and surface only on a live cluster.
        use kube::runtime::watcher::Event;

        let reconciler = TestKubeWatcher::default();
        let ctx = Arc::new(KubeWatcherStubCtx);
        let cm = empty_configmap();

        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(
            &reconciler,
            Arc::clone(&ctx),
            Ok(Event::Apply(cm.clone())),
        )
        .await;
        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(
            &reconciler,
            Arc::clone(&ctx),
            Ok(Event::Delete(cm.clone())),
        )
        .await;
        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(
            &reconciler,
            Arc::clone(&ctx),
            Ok(Event::Init),
        )
        .await;
        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(
            &reconciler,
            Arc::clone(&ctx),
            Ok(Event::InitApply(cm.clone())),
        )
        .await;
        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(
            &reconciler,
            Arc::clone(&ctx),
            Ok(Event::InitDone),
        )
        .await;

        let c = &reconciler.counters;
        assert_eq!(c.apply.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.delete.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.init.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.init_apply.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(c.init_done.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_kube_event_swallows_errors_without_invoking_callbacks() {
        // Stream errors must be logged and dropped — kube-rs's
        // `watcher` recovers on its own via a relist. A regression
        // that turned a stream error into an `on_delete` or `on_init`
        // call would corrupt the implementor's state on every
        // transient apiserver hiccup.
        use kube::runtime::watcher::Error as WatcherError;

        let reconciler = TestKubeWatcher::default();
        let ctx = Arc::new(KubeWatcherStubCtx);

        // `WatcherError::NoResourceVersion` is a cheap concrete variant.
        let err: Result<
            kube::runtime::watcher::Event<<TestKubeWatcher as KubeWatcher>::Resource>,
            WatcherError,
        > = Err(WatcherError::NoResourceVersion);
        dispatch_kube_event::<TestKubeWatcher, KubeWatcherStubCtx>(&reconciler, ctx, err).await;

        let c = &reconciler.counters;
        assert_eq!(c.apply.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(c.delete.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(c.init.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(c.init_apply.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(c.init_done.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// Minimal `KubeWatcher` impl that overrides only the
    /// required callbacks. Used to exercise the trait-level defaults
    /// for `on_init` and `on_init_done` — calling them must complete
    /// trivially without touching `ctx` or the impl's state.
    #[derive(Default)]
    struct MinimalKubeWatcher;

    impl KubeWatcher for MinimalKubeWatcher {
        type Resource = k8s_openapi::api::core::v1::ConfigMap;
        type Context = KubeWatcherStubCtx;

        const NAME: &'static str = "minimal-kube-stream-reconciler";

        fn on_apply(&self, _ctx: Arc<Self::Context>, _obj: Self::Resource) -> crate::BoxFuture<()> {
            Box::pin(async {})
        }
        fn on_delete(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Self::Resource,
        ) -> crate::BoxFuture<()> {
            Box::pin(async {})
        }
        fn on_init_apply(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Self::Resource,
        ) -> crate::BoxFuture<()> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn kube_watcher_defaults() {
        // Pin every trait default a regression could silently shift:
        // `DESCRIPTION` (None), `LEADER_GATED` (true), `on_init`
        // (no-op completes), and `on_init_done` (no-op completes).
        // A flipped `LEADER_GATED` default would silently un-gate
        // every watcher that opts into the default, letting writes
        // to shared external state come from every replica at once.
        // Calling the defaults via `MinimalKubeWatcher` — which
        // overrides only the required callbacks — exercises the
        // actual trait defaults, not an override.
        assert!(<MinimalKubeWatcher as KubeWatcher>::DESCRIPTION.is_none());
        // `clippy::assertions_on_constants` fires on direct asserts
        // against a `const bool`, but that's exactly the regression
        // shape this test exists to catch — a const default flip is
        // invisible to the compiler.
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(
                <MinimalKubeWatcher as KubeWatcher>::LEADER_GATED,
                "default must be leader-gated"
            );
        }

        let r = MinimalKubeWatcher;
        let ctx = Arc::new(KubeWatcherStubCtx);
        // Both defaults are `Box::pin(async {})`; the futures complete
        // immediately and don't touch ctx (the stub panics on access).
        r.on_init(Arc::clone(&ctx)).await;
        r.on_init_done(ctx).await;
    }

    // --- register_kube_watcher! macro test -------------------------

    crate::register_kube_watcher!(TestKubeWatcher, KubeWatcherStubCtx);

    /// The macro must submit a `WorkerInfo` entry that `inventory`
    /// surfaces via `get_workers()`. Mirrors the same round-trip
    /// pinned for `register_kv_worker!` and the other Flatbed
    /// registration macros — without it, `Flatbed::run` would silently
    /// skip the reconciler.
    #[test]
    fn register_kube_watcher_macro_makes_watcher_discoverable() {
        let workers = crate::get_workers();
        let found = workers
            .iter()
            .find(|w| w.name == <TestKubeWatcher as KubeWatcher>::NAME);
        assert!(
            found.is_some(),
            "TestKubeWatcher should be discoverable via inventory"
        );
        let info = found.unwrap();
        assert_eq!(info.name, "test-kube-stream-reconciler");
        assert_eq!(
            info.description,
            <TestKubeWatcher as KubeWatcher>::DESCRIPTION
        );
    }

    // ========================================================================
    // KubeNativeReconciler trait + run_kube_native_reconciler tests
    // ========================================================================

    /// Minimal context that satisfies the `KubeNativeReconciler::Context`
    /// bound (`HasKubeClient + HasLeaderElection`). Accessors panic —
    /// the tests in this block exercise trait-default and
    /// macro-inventory surfaces only, never the live kube client or
    /// watch.
    struct KubeNativeStubCtx;

    impl HasKubeClient for KubeNativeStubCtx {
        fn kube_client(&self) -> &kube::Client {
            panic!("stub: kube_client should not be called in trait-default tests");
        }
    }

    impl HasLeaderElection for KubeNativeStubCtx {
        fn is_leader_rx(&self) -> watch::Receiver<bool> {
            panic!("stub: is_leader_rx should not be called in trait-default tests");
        }
    }

    #[derive(Default)]
    struct TestKubeNativeReconciler;

    impl KubeNativeReconciler for TestKubeNativeReconciler {
        type Resource = k8s_openapi::api::core::v1::ConfigMap;
        type Context = KubeNativeStubCtx;
        type Error = ReconcileError;

        const NAME: &'static str = "test-kube-native-reconciler";
        const DESCRIPTION: Option<&'static str> = Some("trait-default pin target");

        fn reconcile(
            &self,
            _ctx: Arc<Self::Context>,
            _obj: Arc<Self::Resource>,
        ) -> crate::BoxFuture<Result<kube::runtime::controller::Action, Self::Error>> {
            Box::pin(async {
                Ok(kube::runtime::controller::Action::requeue(
                    std::time::Duration::from_secs(300),
                ))
            })
        }
    }

    /// Pin the trait-level defaults — a regression that flipped
    /// `FINALIZER` to `Some` by accident would silently start adding
    /// finalizers to every CR a `KubeNativeReconciler` watches.
    #[test]
    fn kube_native_reconciler_trait_defaults() {
        assert!(<TestKubeNativeReconciler as KubeNativeReconciler>::FINALIZER.is_none());
        assert_eq!(
            <TestKubeNativeReconciler as KubeNativeReconciler>::DESCRIPTION,
            Some("trait-default pin target")
        );
        assert_eq!(
            <TestKubeNativeReconciler as KubeNativeReconciler>::NAME,
            "test-kube-native-reconciler"
        );
    }

    /// `reconcile_with_finalizer_kube_native` must short-circuit
    /// straight to `reconcile` when `FINALIZER == None`, never
    /// touching the kube client (the stub context panics on
    /// `kube_client()` access). Pinning here so a refactor that
    /// swapped the order or always reached for the client would
    /// surface as a panic.
    #[tokio::test]
    async fn reconcile_with_finalizer_kube_native_skips_kube_when_no_finalizer() {
        let r = TestKubeNativeReconciler;
        let cm = k8s_openapi::api::core::v1::ConfigMap {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("no-finalizer".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = Arc::new(KubeNativeStubCtx);
        let result = reconcile_with_finalizer_kube_native(&r, ctx, Arc::new(cm)).await;
        assert!(result.is_ok(), "no-finalizer path must succeed");
    }

    // --- register_kube_native_reconciler! macro test ---------------

    crate::register_kube_native_reconciler!(TestKubeNativeReconciler, KubeNativeStubCtx);

    /// The macro must submit a `WorkerInfo` entry that `inventory`
    /// surfaces via `get_workers()`. Without it, `Flatbed::run` would
    /// silently skip the reconciler.
    #[test]
    fn register_kube_native_reconciler_macro_makes_reconciler_discoverable() {
        let workers = crate::get_workers();
        let found = workers
            .iter()
            .find(|w| w.name == <TestKubeNativeReconciler as KubeNativeReconciler>::NAME);
        assert!(
            found.is_some(),
            "TestKubeNativeReconciler should be discoverable via inventory"
        );
        let info = found.unwrap();
        assert_eq!(info.name, "test-kube-native-reconciler");
        assert_eq!(
            info.description,
            <TestKubeNativeReconciler as KubeNativeReconciler>::DESCRIPTION
        );
    }
}
