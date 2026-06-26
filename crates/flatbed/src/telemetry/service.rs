use std::{collections::HashMap, sync::Arc};

use super::{Counter, GaugeVec, TelemetryConfig, TelemetryError};

/// Trait for telemetry services that collect and expose metrics.
pub trait TelemetryService: Send + Sync {
    /// Create a new telemetry service with the given configuration.
    fn new(config: TelemetryConfig) -> Arc<Self>
    where
        Self: Sized;

    /// Register a counter metric with f64 values.
    #[allow(dead_code)]
    fn register_f64_counter(
        &self,
        name: &str,
        help: &str,
        labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<f64>>, TelemetryError>;

    /// Register a counter metric with u64 values.
    fn register_u64_counter(
        &self,
        name: &str,
        help: &str,
        labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<u64>>, TelemetryError>;

    /// Register a labeled gauge metric with u64 values. `label_names`
    /// is the schema for the labels every `set` call must supply, in
    /// order. Const labels (`service` / `ip_address` from
    /// `TelemetryConfig`) are attached to the metric descriptor by
    /// the backend automatically and must **not** be passed to
    /// `set` — only the schema's `label_names` count toward the
    /// arity check.
    ///
    /// Defaults to `Err` so counter-only implementations compile
    /// without overriding this method.
    fn register_u64_gauge_vec(
        &self,
        _name: &str,
        _help: &str,
        _label_names: &[&str],
    ) -> Result<Arc<dyn GaugeVec<u64>>, TelemetryError> {
        Err(TelemetryError::RegisterGaugeError(
            "register_u64_gauge_vec not implemented for this TelemetryService backend".to_string(),
        ))
    }

    /// Get the metrics feed in Prometheus text format.
    fn get_feed(&self) -> Result<String, TelemetryError>;

    /// Get the service name.
    fn service_name(&self) -> String;

    /// Get the IP address.
    fn ip_address(&self) -> String;
}
