//! Optional context wrapper that owns framework plumbing.
//!
//! [`FlatbedContext<C>`] is a convenience wrapper for services that use NATS
//! and/or Kubernetes. It wraps the user's application data `C`, owns the
//! framework clients, and auto-implements the required traits
//! ([`HasJetStream`], [`HasKubeClient`], [`HasLeaderElection`]).
//!
//! **This is entirely optional.** `Flatbed::run` accepts any
//! `C: Clone + Send + Sync + 'static` — you can define your own context
//! struct and implement the traits manually if you need more control.
//!
//! Which fields and trait impls are included depends on enabled features:
//!
//! | Feature        | Fields added                                              | Traits implemented    |
//! |----------------|-----------------------------------------------------------|-----------------------|
//! | `nats`         | `nats_client`, `jetstream`, `stream_replicas`             | `HasJetStream`        |
//! | `k8s`          | `kube_client`, leader election fields                     | `HasKubeClient`, `HasLeaderElection` |
//!
//! User fields on `C` are accessible transparently via [`Deref`].
//!
//! # Example: custom context (no wrapper)
//!
//! ```rust,ignore
//! #[derive(Clone)]
//! struct AppContext {
//!     pub jetstream: async_nats::jetstream::Context,
//!     pub db: DatabasePool,
//! }
//!
//! impl flatbed::HasJetStream for AppContext {
//!     fn jetstream(&self) -> &async_nats::jetstream::Context {
//!         &self.jetstream
//!     }
//! }
//!
//! Flatbed::run(config, |_| async move {
//!     Ok(AppContext { jetstream, db })
//! }).await?;
//! ```
//!
//! # Example: FlatbedContext with nats + k8s
//!
//! ```rust,ignore
//! #[derive(Clone)]
//! struct AppData {
//!     pub namespace: String,
//! }
//!
//! type AppContext = flatbed::FlatbedContext<AppData>;
//!
//! let ctx = FlatbedContext::builder(AppData { namespace: "default".into() })
//!     .nats_client(nats_client)
//!     .jetstream(jetstream)
//!     .kube_client(kube_client)
//!     .leader_election(is_leader_tx, is_leader_rx, ha_mode)
//!     .build();
//!
//! // User fields via Deref:
//! println!("{}", ctx.namespace);
//!
//! // Framework fields directly:
//! ctx.jetstream.publish("subject", payload).await?;
//! ```
//!
//! # Example: FlatbedContext with nats only
//!
//! ```rust,ignore
//! let ctx = FlatbedContext::builder(AppData { ... })
//!     .nats_client(nats_client)
//!     .jetstream(jetstream)
//!     .build();
//! ```

use std::ops::Deref;
#[cfg(feature = "k8s")]
use std::sync::Arc;

#[cfg(feature = "k8s")]
use tokio::sync::watch;

/// Optional context wrapper that owns framework plumbing.
///
/// Wraps the user's application data `C` and auto-implements framework
/// traits based on enabled features. Access user fields via [`Deref`].
///
/// This is a convenience — you can always define your own context struct
/// and implement the traits manually instead. See the [module docs](self)
/// for examples of both approaches.
#[derive(Clone)]
pub struct FlatbedContext<C> {
    inner: C,

    /// NATS client connection.
    #[cfg(feature = "nats")]
    pub nats_client: async_nats::Client,
    /// JetStream context for publishing and consuming messages.
    #[cfg(feature = "nats")]
    pub jetstream: async_nats::jetstream::Context,
    /// Number of replicas to create JetStream streams with.
    /// Defaults to 1; clustered deployments set this to match the
    /// number of NATS peers so streams survive a single peer loss.
    #[cfg(feature = "nats")]
    pub stream_replicas: usize,

    /// Kubernetes API client.
    #[cfg(feature = "k8s")]
    pub kube_client: kube::Client,
    /// Sender for leader election state changes.
    #[cfg(feature = "k8s")]
    pub is_leader_tx: Arc<watch::Sender<bool>>,
    #[cfg(feature = "k8s")]
    is_leader_rx: watch::Receiver<bool>,
    /// Whether HA (leader election) mode is enabled.
    #[cfg(feature = "k8s")]
    pub ha_mode: bool,
}

impl<C> Deref for FlatbedContext<C> {
    type Target = C;

    fn deref(&self) -> &C {
        &self.inner
    }
}

#[cfg(feature = "nats")]
impl<C> crate::nats::HasJetStream for FlatbedContext<C> {
    fn jetstream(&self) -> &async_nats::jetstream::Context {
        &self.jetstream
    }

    fn stream_replicas(&self) -> usize {
        self.stream_replicas
    }
}

#[cfg(feature = "k8s")]
impl<C> crate::k8s::HasKubeClient for FlatbedContext<C> {
    fn kube_client(&self) -> &kube::Client {
        &self.kube_client
    }
}

#[cfg(feature = "k8s")]
impl<C> crate::k8s::HasLeaderElection for FlatbedContext<C> {
    fn is_leader_rx(&self) -> watch::Receiver<bool> {
        self.is_leader_rx.clone()
    }

    fn ha_enabled(&self) -> bool {
        self.ha_mode
    }
}

impl<C> FlatbedContext<C> {
    /// Start building a new `FlatbedContext` wrapping the given application data.
    pub fn builder(inner: C) -> FlatbedContextBuilder<C> {
        FlatbedContextBuilder {
            inner,
            #[cfg(feature = "nats")]
            nats_client: None,
            #[cfg(feature = "nats")]
            jetstream: None,
            #[cfg(feature = "nats")]
            stream_replicas: None,
            #[cfg(feature = "k8s")]
            kube_client: None,
            #[cfg(feature = "k8s")]
            is_leader_tx: None,
            #[cfg(feature = "k8s")]
            is_leader_rx: None,
            #[cfg(feature = "k8s")]
            ha_mode: None,
        }
    }

    /// Returns a reference to the inner application data.
    pub fn inner(&self) -> &C {
        &self.inner
    }
}

/// Builder for [`FlatbedContext`].
///
/// Created via [`FlatbedContext::builder`]. Call setter methods for each
/// enabled feature, then `.build()` to construct the context.
///
/// # Panics
///
/// `.build()` panics if any required field for an enabled feature is missing.
pub struct FlatbedContextBuilder<C> {
    inner: C,

    #[cfg(feature = "nats")]
    nats_client: Option<async_nats::Client>,
    #[cfg(feature = "nats")]
    jetstream: Option<async_nats::jetstream::Context>,
    #[cfg(feature = "nats")]
    stream_replicas: Option<usize>,

    #[cfg(feature = "k8s")]
    kube_client: Option<kube::Client>,
    #[cfg(feature = "k8s")]
    is_leader_tx: Option<Arc<watch::Sender<bool>>>,
    #[cfg(feature = "k8s")]
    is_leader_rx: Option<watch::Receiver<bool>>,
    #[cfg(feature = "k8s")]
    ha_mode: Option<bool>,
}

impl<C> FlatbedContextBuilder<C> {
    /// Set the NATS client connection.
    #[cfg(feature = "nats")]
    pub fn nats_client(mut self, nats_client: async_nats::Client) -> Self {
        self.nats_client = Some(nats_client);
        self
    }

    /// Set the JetStream context.
    #[cfg(feature = "nats")]
    pub fn jetstream(mut self, jetstream: async_nats::jetstream::Context) -> Self {
        self.jetstream = Some(jetstream);
        self
    }

    /// Set the JetStream stream replica count.
    ///
    /// Defaults to 1 when not set — single-server JetStream. Set to
    /// the NATS peer count when running against a clustered
    /// deployment so reconciler-controller streams survive a
    /// single peer loss.
    #[cfg(feature = "nats")]
    pub fn stream_replicas(mut self, n: usize) -> Self {
        self.stream_replicas = Some(n);
        self
    }

    /// Set the Kubernetes client.
    #[cfg(feature = "k8s")]
    pub fn kube_client(mut self, kube_client: kube::Client) -> Self {
        self.kube_client = Some(kube_client);
        self
    }

    /// Set leader election configuration.
    ///
    /// - `is_leader_tx`: Sender for leader state updates
    /// - `is_leader_rx`: Receiver for watching leader state
    /// - `ha_mode`: Whether HA (leader election) is enabled
    #[cfg(feature = "k8s")]
    pub fn leader_election(
        mut self,
        is_leader_tx: Arc<watch::Sender<bool>>,
        is_leader_rx: watch::Receiver<bool>,
        ha_mode: bool,
    ) -> Self {
        self.is_leader_tx = Some(is_leader_tx);
        self.is_leader_rx = Some(is_leader_rx);
        self.ha_mode = Some(ha_mode);
        self
    }

    /// Build the [`FlatbedContext`].
    ///
    /// # Panics
    ///
    /// Panics if any required field for an enabled feature was not set.
    pub fn build(self) -> FlatbedContext<C> {
        FlatbedContext {
            inner: self.inner,
            #[cfg(feature = "nats")]
            nats_client: self.nats_client.expect("nats_client is required"),
            #[cfg(feature = "nats")]
            jetstream: self.jetstream.expect("jetstream is required"),
            #[cfg(feature = "nats")]
            stream_replicas: self.stream_replicas.unwrap_or(1),
            #[cfg(feature = "k8s")]
            kube_client: self.kube_client.expect("kube_client is required"),
            #[cfg(feature = "k8s")]
            is_leader_tx: self.is_leader_tx.expect("leader_election is required"),
            #[cfg(feature = "k8s")]
            is_leader_rx: self.is_leader_rx.expect("leader_election is required"),
            #[cfg(feature = "k8s")]
            ha_mode: self.ha_mode.expect("leader_election is required"),
        }
    }
}
