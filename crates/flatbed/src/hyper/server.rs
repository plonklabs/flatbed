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
/// Workers are deferred until the server is marked as ready via the ready channel.
/// If a worker fails, the healthz channel is set to false to trigger Kubernetes restarts.
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
    /// Workers are spawned only after the ready signal is received.
    /// If a worker fails, the healthz channel is set to false.
    pub async fn serve(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // Spawn shutdown signal handler
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = shutdown_tx_clone.send(true);
        });

        // Spawn worker launcher that waits for ready signal
        let mut ready_rx = self.service_ctx.ready_rx.clone();
        let context = Arc::clone(&self.service_ctx.context);
        let healthz_tx = self.healthz_tx.clone();
        let shutdown_tx_for_workers = shutdown_tx.clone();

        tokio::spawn(async move {
            // Wait for ready signal
            loop {
                if *ready_rx.borrow() {
                    break;
                }
                if ready_rx.changed().await.is_err() {
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
                let name = worker_info.name.to_string();
                let worker_fn = worker_info.worker;
                let healthz_tx = healthz_tx.clone();
                let shutdown_tx = shutdown_tx_for_workers.clone();
                let ctx = worker_ctx.clone();

                tokio::spawn(async move {
                    if let Err(e) = worker_fn(ctx).await {
                        error!(worker = %name, error = %e, "worker failed");
                        // Mark server as unhealthy so Kubernetes restarts the pod
                        let _ = healthz_tx.send(false);
                        // Trigger graceful shutdown on worker failure
                        let _ = shutdown_tx.send(true);
                    }
                });
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

        // Run registered drain functions so workers can finish in-progress work
        let drains = get_worker_drains();
        if !drains.is_empty() {
            let ctx_guard = service_ctx.context.read().await;
            let Some(app_ctx) = ctx_guard.as_ref() else {
                warn!("context not initialised; skipping worker drains");
                drop(ctx_guard);
                tokio::time::sleep(tokio::time::Duration::from_secs(self.shutdown_timeout_secs))
                    .await;
                return Ok(());
            };

            let worker_ctx: Arc<dyn std::any::Any + Send + Sync> = Arc::clone(app_ctx) as _;
            drop(ctx_guard);

            let mut drain_handles = Vec::with_capacity(drains.len());
            for drain_info in &drains {
                let name = drain_info.name.to_string();
                let ctx = worker_ctx.clone();
                let drain_fn = drain_info.drain;
                drain_handles.push(tokio::spawn(async move {
                    info!(worker = %name, "draining worker");
                    if let Err(e) = drain_fn(ctx).await {
                        error!(worker = %name, error = %e, "drain failed");
                    } else {
                        info!(worker = %name, "drain complete");
                    }
                }));
            }

            let drain_timeout = tokio::time::Duration::from_secs(
                self.shutdown_timeout_secs.saturating_sub(2).max(1),
            );
            let _ = tokio::time::timeout(drain_timeout, async {
                for handle in drain_handles {
                    let _ = handle.await;
                }
            })
            .await;
        }

        // Allow in-flight connections to complete.
        // After drains ran, a brief epilogue (2s) is enough. If no drains
        // were registered, use the full shutdown budget for connections.
        let connection_drain_secs = if drains.is_empty() {
            self.shutdown_timeout_secs
        } else {
            self.shutdown_timeout_secs.min(2)
        };
        tokio::time::sleep(tokio::time::Duration::from_secs(connection_drain_secs)).await;

        Ok(())
    }
}
