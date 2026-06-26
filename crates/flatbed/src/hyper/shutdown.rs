//! Graceful shutdown signal handling
//!
//! Handles SIGTERM and SIGINT (Ctrl+C) for graceful server shutdown.

/// Wait for a shutdown signal (SIGTERM or SIGINT)
///
/// This function returns when either signal is received, allowing
/// the server to perform graceful shutdown.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Shutdown controller for programmatic shutdown
///
/// Allows triggering shutdown from code (e.g., from a test or management endpoint).
pub struct ShutdownController {
    tx: tokio::sync::watch::Sender<bool>,
    rx: tokio::sync::watch::Receiver<bool>,
}

impl ShutdownController {
    /// Create a new shutdown controller
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(false);
        Self { tx, rx }
    }

    /// Get a receiver that can be used to wait for shutdown
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.rx.clone()
    }

    /// Trigger shutdown
    pub fn shutdown(&self) {
        let _ = self.tx.send(true);
    }

    /// Wait for shutdown signal
    pub async fn wait(&mut self) {
        while !*self.rx.borrow() {
            if self.rx.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_controller() {
        let controller = ShutdownController::new();
        let mut rx = controller.subscribe();

        // Initially not shutdown
        assert!(!*rx.borrow());

        // Trigger shutdown
        controller.shutdown();

        // Should be shutdown now
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
    }
}
