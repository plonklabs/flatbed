//! Runtime readiness gates.
//!
//! Readiness has two parts. The boot latch is one-shot: it flips once, when
//! the boot function has returned a context, and never flips back. A *gate*
//! is the runtime half — a named dependency that can come and go for the
//! rest of the process's life, such as a broker connection, a database pool,
//! or a leader lease. The server reports ready only when the boot latch is
//! set and every registered gate is ready.
//!
//! A gate starts *not ready*. Registering one is a statement that the process
//! is not ready until whoever owns the dependency says otherwise.
//!
//! ```rust,ignore
//! use flatbed::{Flatbed, FlatbedConfig};
//!
//! let config = FlatbedConfig::new("My API").with_telemetry(telemetry);
//!
//! Flatbed::run(config, |cfg| async move {
//!     let nats = flatbed::nats::Connector::new("nats://broker:4222")
//!         .readiness(cfg.readiness.gate("nats"))
//!         .connect_with_retry()
//!         .await?;
//!     Ok(AppContext { nats })
//! })
//! .await
//! ```

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

struct GateState {
    name: String,
    ready: AtomicBool,
    blocking: Arc<AtomicUsize>,
}

/// The registry of runtime dependencies that readiness answers on, alongside
/// the boot latch.
///
/// Clones share one registry, so the handle a boot function reaches through
/// [`FlatbedConfig`](crate::FlatbedConfig) drives the same probe the server
/// answers from.
#[derive(Clone, Default)]
pub struct Readiness {
    gates: Arc<Mutex<Vec<Arc<GateState>>>>,
    blocking: Arc<AtomicUsize>,
}

impl Readiness {
    /// An empty registry, which is ready until the first gate is added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a runtime dependency under `name`, starting not ready.
    ///
    /// A gate lasts for the life of the process: dropping every handle leaves
    /// the gate at its last value rather than removing it, so a dependency
    /// whose owner disappears keeps the process out of the load balancer
    /// instead of silently reporting healthy.
    pub fn gate(&self, name: impl Into<String>) -> ReadinessGate {
        let state = Arc::new(GateState {
            name: name.into(),
            ready: AtomicBool::new(false),
            blocking: Arc::clone(&self.blocking),
        });
        self.blocking.fetch_add(1, Ordering::AcqRel);
        self.lock().push(Arc::clone(&state));
        ReadinessGate { state }
    }

    /// Whether every registered gate is ready.
    ///
    /// Every request answered by the server consults this, so it reads the
    /// count of gates that are not ready rather than walking the registry
    /// behind its lock.
    pub fn is_ready(&self) -> bool {
        self.blocking.load(Ordering::Acquire) == 0
    }

    /// The names of the gates currently holding readiness down, in
    /// registration order, for probe bodies and logs.
    pub fn blocked_on(&self) -> Vec<String> {
        self.lock()
            .iter()
            .filter(|gate| !gate.ready.load(Ordering::Acquire))
            .map(|gate| gate.name.clone())
            .collect()
    }

    /// The guarded value is a list of independent atomics, so no panic can
    /// leave it half-updated and a poisoned lock is recovered rather than
    /// propagated — a probe that panics forever is worse than one that reads
    /// a consistent value.
    fn lock(&self) -> MutexGuard<'_, Vec<Arc<GateState>>> {
        self.gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl fmt::Debug for Readiness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_map();
        for gate in self.lock().iter() {
            debug.entry(&gate.name, &gate.ready.load(Ordering::Acquire));
        }
        debug.finish()
    }
}

/// A handle to one registered runtime dependency.
///
/// Clones address the same gate. The registry holds its own reference, so the
/// gate outlives every handle.
#[derive(Clone)]
pub struct ReadinessGate {
    state: Arc<GateState>,
}

impl ReadinessGate {
    /// The name the gate was registered under.
    pub fn name(&self) -> &str {
        &self.state.name
    }

    /// Report whether the dependency is currently usable.
    pub fn set_ready(&self, ready: bool) {
        if self.state.ready.swap(ready, Ordering::AcqRel) == ready {
            return;
        }

        match ready {
            true => self.state.blocking.fetch_sub(1, Ordering::AcqRel),
            false => self.state.blocking.fetch_add(1, Ordering::AcqRel),
        };
    }

    /// Whether this gate is currently ready.
    pub fn is_ready(&self) -> bool {
        self.state.ready.load(Ordering::Acquire)
    }
}

impl fmt::Debug for ReadinessGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadinessGate")
            .field("name", &self.state.name)
            .field("ready", &self.is_ready())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::Readiness;

    #[test]
    fn empty_registry_is_ready() {
        let readiness = Readiness::new();

        assert!(readiness.is_ready());
        assert!(readiness.blocked_on().is_empty());
    }

    #[test]
    fn a_new_gate_starts_not_ready() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");

        assert!(!gate.is_ready());
        assert!(!readiness.is_ready());
        assert_eq!(readiness.blocked_on(), vec!["nats".to_string()]);
    }

    #[test]
    fn readiness_needs_every_gate() {
        let readiness = Readiness::new();
        let nats = readiness.gate("nats");
        let db = readiness.gate("db");

        nats.set_ready(true);
        assert!(!readiness.is_ready());
        assert_eq!(readiness.blocked_on(), vec!["db".to_string()]);

        db.set_ready(true);
        assert!(readiness.is_ready());
        assert!(readiness.blocked_on().is_empty());
    }

    #[test]
    fn a_gate_can_flip_back() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");

        gate.set_ready(true);
        assert!(readiness.is_ready());

        gate.set_ready(false);
        assert!(!readiness.is_ready());
        assert_eq!(readiness.blocked_on(), vec!["nats".to_string()]);
    }

    #[test]
    fn repeating_a_gate_value_leaves_readiness_where_it_was() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");

        gate.set_ready(true);
        gate.set_ready(true);
        assert!(readiness.is_ready());

        gate.set_ready(false);
        assert!(!readiness.is_ready());
    }

    #[test]
    fn concurrent_flips_leave_readiness_consistent() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");
        gate.set_ready(true);

        let flippers: Vec<_> = (0..8)
            .map(|_| {
                let gate = gate.clone();
                std::thread::spawn(move || {
                    for _ in 0..1_000 {
                        gate.set_ready(false);
                        gate.set_ready(true);
                    }
                })
            })
            .collect();

        for flipper in flippers {
            flipper.join().expect("flipper thread");
        }

        assert!(readiness.is_ready());
        assert!(readiness.blocked_on().is_empty());
    }

    #[test]
    fn clones_share_one_registry() {
        let readiness = Readiness::new();
        let gate = readiness.gate("nats");
        let observer = readiness.clone();

        assert!(!observer.is_ready());
        gate.set_ready(true);
        assert!(observer.is_ready());
    }

    #[test]
    fn a_gate_outlives_its_handles() {
        let readiness = Readiness::new();
        drop(readiness.gate("nats"));

        assert!(!readiness.is_ready());
        assert_eq!(readiness.blocked_on(), vec!["nats".to_string()]);
    }
}
