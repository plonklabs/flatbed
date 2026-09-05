//! HTTP server implementation using hyper
//!
//! Supports both HTTP/1.1 and HTTP/2 cleartext (no TLS - Envoy handles it).

use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::watch;

use tracing::{debug, error, info, warn};

use super::service::{FlatbedService, ServiceContext};
use super::shutdown::shutdown_signal;
use crate::supervisor::supervise;
use crate::{get_worker_drains, get_workers};

/// Tokio executor for hyper HTTP/2
#[derive(Clone, Copy)]
struct TokioExecutor;

impl<F> hyper::rt::Executor<F> for TokioExecutor
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        tokio::spawn(fut);
    }
}

/// Auto-detecting server that handles both HTTP/1.1 and HTTP/2
///
/// This server automatically detects the protocol based on the connection preface.
/// HTTP/2 connections start with "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".
///
/// Workers are deferred until the server is marked as ready via the ready
/// channel, then run under the supervisor, which sets the healthz channel to
/// false when one ends terminally so Kubernetes restarts the pod.
pub struct AutoServer<C> {
    bind_addr: SocketAddr,
    service_ctx: ServiceContext<C>,
    healthz_tx: watch::Sender<bool>,
    shutdown_timeout_secs: u64,
}

impl<C: Clone + Send + Sync + 'static> AutoServer<C> {
    /// Create a new auto-detecting server
    ///
    /// The server uses the ServiceContext for health/ready state management.
    /// The healthz_tx is used to mark the server as unhealthy when workers fail.
    pub fn new(
        bind_addr: SocketAddr,
        service_ctx: ServiceContext<C>,
        healthz_tx: watch::Sender<bool>,
    ) -> Self {
        Self {
            bind_addr,
            service_ctx,
            healthz_tx,
            shutdown_timeout_secs: 30,
        }
    }

    /// Set the graceful shutdown timeout in seconds
    pub fn shutdown_timeout(mut self, secs: u64) -> Self {
        self.shutdown_timeout_secs = secs;
        self
    }

    /// Start the server and run until shutdown signal
    ///
    /// Automatically handles both HTTP/1.1 and HTTP/2 connections.
    /// Workers are spawned under supervision once the ready signal arrives,
    /// and registered drains run within the shutdown budget.
    pub async fn serve(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;

        #[cfg(feature = "telemetry")]
        if let Some(telemetry) = self.service_ctx.config.telemetry.as_ref() {
            crate::supervisor::install_state_gauge(telemetry);
        }

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // Spawn shutdown signal handler
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = shutdown_tx_clone.send(true);
        });

        // Workers start on the boot latch alone: a worker whose job is to
        // establish a gated dependency would never start if it waited on the
        // gates.
        let mut booted_rx = self.service_ctx.booted_rx.clone();
        let context = Arc::clone(&self.service_ctx.context);
        let healthz_tx = self.healthz_tx.clone();
        let shutdown_tx_for_workers = shutdown_tx.clone();

        tokio::spawn(async move {
            loop {
                if *booted_rx.borrow() {
                    break;
                }
                if booted_rx.changed().await.is_err() {
                    return; // Channel closed, server shutting down
                }
            }

            // Get the context (should be set by now)
            let ctx_guard = context.read().await;
            let Some(app_ctx) = ctx_guard.as_ref() else {
                warn!("ready signal received but context not set");
                return;
            };

            // Spawn registered workers with app context
            // Workers receive Arc<dyn Any + Send + Sync> and downcast to their expected type
            let cloned: Arc<C> = Arc::clone(app_ctx);
            drop(ctx_guard); // Release the read lock
            let worker_ctx: Arc<dyn Any + Send + Sync> = cloned;

            let workers = get_workers();
            for w in &workers {
                info!(
                    name = w.name,
                    description = w.description.unwrap_or("none"),
                    "registered worker"
                );
            }

            for worker_info in workers {
                tokio::spawn(supervise(
                    *worker_info,
                    worker_ctx.clone(),
                    healthz_tx.clone(),
                    shutdown_tx_for_workers.clone(),
                ));
            }
        });

        let service_ctx = self.service_ctx;

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _addr) = result?;
                    let service = FlatbedService::new(service_ctx.clone());
                    let mut conn_shutdown_rx = shutdown_rx.clone();

                    tokio::spawn(async move {
                        // Use hyper_util's auto connection builder
                        let io = TokioIo::new(stream);

                        let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor);
                        let conn = builder.serve_connection(io, service);

                        tokio::select! {
                            result = conn => {
                                if let Err(e) = result {
                                    debug!(error = %e, "connection error");
                                }
                            }
                            _ = conn_shutdown_rx.changed() => {
                                // Graceful shutdown
                            }
                        }
                    });
                }
                _ = shutdown_rx.changed() => {
                    break;
                }
            }
        }

        // The shutdown budget covers draining workers and then letting
        // in-flight connections finish; whatever the drains do not spend is
        // left to the connections.
        let deadline = tokio::time::Instant::now()
            + tokio::time::Duration::from_secs(self.shutdown_timeout_secs);
        run_worker_drains(&service_ctx, deadline).await;
        tokio::time::sleep_until(deadline).await;

        Ok(())
    }
}

/// Reserve the last two seconds of the shutdown budget for in-flight
/// connections and give the rest to the registered worker drains.
const CONNECTION_EPILOGUE: tokio::time::Duration = tokio::time::Duration::from_secs(2);

async fn run_worker_drains<C: Clone + Send + Sync + 'static>(
    service_ctx: &ServiceContext<C>,
    deadline: tokio::time::Instant,
) {
    let drains = get_worker_drains();
    if drains.is_empty() {
        return;
    }

    let worker_ctx: Arc<dyn Any + Send + Sync> = {
        let guard = service_ctx.context.read().await;
        let Some(app_ctx) = guard.as_ref() else {
            warn!("context not initialised; skipping worker drains");
            return;
        };
        Arc::clone(app_ctx) as Arc<dyn Any + Send + Sync>
    };

    let handles: Vec<_> = drains
        .iter()
        .map(|drain_info| {
            let name = drain_info.name;
            let ctx = worker_ctx.clone();
            let drain_fn = drain_info.drain;
            let handle = tokio::spawn(async move {
                if let Err(e) = drain_fn(ctx).await {
                    error!(worker = name, error = %e, "drain failed");
                } else {
                    debug!(worker = name, "drain complete");
                }
            });
            (name, handle)
        })
        .collect();

    let drain_deadline = deadline
        .checked_sub(CONNECTION_EPILOGUE)
        .unwrap_or(deadline);
    let _ = tokio::time::timeout_at(drain_deadline, async {
        for (name, handle) in handles {
            if let Err(e) = handle.await {
                error!(worker = name, error = %e, "drain task did not complete");
            }
        }
    })
    .await;
}
