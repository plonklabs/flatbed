//! Telemetry module for health endpoints and metrics.
//!
//! Defines the [`TelemetryService`] trait + supporting types
//! ([`Counter`], [`Gauge`], [`GaugeVec`], [`MetricType`],
//! [`MetricValue`], [`TelemetryError`], [`TelemetryConfig`])
//! consumers implement to plug a backend into a flatbed service. A
//! Prometheus-backed implementation ships behind the `prometheus`
//! feature flag at [`prometheus::PrometheusTelemetryService`];
//! it covers counters and labeled gauges. Consumers needing
//! histograms supply their own backend.

mod config;
#[cfg(feature = "prometheus")]
pub mod prometheus;
mod service;
mod types;

pub use config::TelemetryConfig;
pub use service::TelemetryService;
pub use types::Counter;
pub use types::Gauge;
pub use types::GaugeVec;
pub use types::MetricType;
pub use types::MetricValue;
pub use types::TelemetryError;
