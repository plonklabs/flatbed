use std::fmt::Display;

/// Trait for counter metrics.
pub trait Counter<T: MetricValue>: Send + Sync {
    /// Increments the counter by 1.
    fn inc(&self);
    /// Increments the counter by a given value.
    fn inc_by(&self, value: T);
}

/// Trait for gauge metrics.
#[allow(dead_code)]
pub trait Gauge<T: MetricValue>: Send + Sync {
    /// Sets the gauge to a specific value.
    fn set(&self, value: T);
    /// Increments the gauge by a given value.
    fn inc_by(&self, value: T);
    /// Decrements the gauge by a given value.
    fn dec_by(&self, value: T);
}

/// Trait for labeled gauge metrics — one metric name, many label-value
/// combinations. Callers `set` per label tuple; `remove_label_values`
/// drops a single combination so a periodic collector can prune only
/// the combinations that have actually disappeared (keeping the
/// surviving ones continuously present in the `/metrics` feed, with
/// no scrape-window gap where the metric goes empty); `reset` drops
/// every combination at once (e.g. on shutdown / leadership loss).
pub trait GaugeVec<T: MetricValue>: Send + Sync {
    /// Set the gauge child for `label_values` to `value`. The number
    /// and order of `label_values` must match the `label_names` the
    /// vec was registered with; mismatches surface as a logged error
    /// and skip the update rather than panic.
    fn set(&self, label_values: &[&str], value: T);

    /// Drop the child tracked under `label_values`. After this call
    /// the `/metrics` feed emits no series for that label tuple
    /// until the next `set`. Mismatched arity is logged and ignored.
    fn remove_label_values(&self, label_values: &[&str]);

    /// Drop every label-value child this vec has tracked. After
    /// `reset`, the `/metrics` feed emits no series for this vec
    /// until the next `set`.
    fn reset(&self);
}

/// Trait for metric value types.
pub trait MetricValue: Send + Sync {
    const METRIC_TYPE: MetricType;
}

/// Supported metric value types.
pub enum MetricType {
    F64,
    U64,
    I64,
}

impl MetricValue for f64 {
    const METRIC_TYPE: MetricType = MetricType::F64;
}

impl MetricValue for u64 {
    const METRIC_TYPE: MetricType = MetricType::U64;
}

impl MetricValue for i64 {
    const METRIC_TYPE: MetricType = MetricType::I64;
}

/// Errors that can occur in the telemetry module.
#[derive(Debug)]
pub enum TelemetryError {
    /// Error when trying to register a counter.
    RegisterCounterError(String),
    /// Error when trying to register a gauge or labeled gauge.
    RegisterGaugeError(String),
    /// Error when trying to encode metrics.
    EncodeMetricsError(String),
    /// Error when validating the telemetry configuration.
    ConfigValidationError(String),
}

impl Display for TelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelemetryError::RegisterCounterError(msg) => {
                write!(f, "Error registering counter: {}", msg)
            }
            TelemetryError::RegisterGaugeError(msg) => {
                write!(f, "Error registering gauge: {}", msg)
            }
            TelemetryError::EncodeMetricsError(msg) => {
                write!(f, "Error encoding metrics: {}", msg)
            }
            TelemetryError::ConfigValidationError(msg) => {
                write!(f, "Error validating telemetry configuration: {}", msg)
            }
        }
    }
}

impl std::error::Error for TelemetryError {}
