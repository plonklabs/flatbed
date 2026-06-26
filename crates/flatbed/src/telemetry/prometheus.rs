//! [`TelemetryService`] implementation backed by a
//! [`prometheus::Registry`], available via the `prometheus` feature
//! flag. Supports counters (`f64`/`u64`) and labeled gauges
//! (`u64`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::telemetry::{Counter, GaugeVec, TelemetryConfig, TelemetryError, TelemetryService};
use prometheus::core::{Atomic, AtomicF64, AtomicU64};
use prometheus::TextEncoder;
use tracing::warn;

/// Wraps a [`prometheus::Registry`] behind the [`TelemetryService`]
/// trait. One instance per service — the registry is per-process and
/// the const-labels are baked from the config's service name + IP.
pub struct PrometheusTelemetryService {
    config: TelemetryConfig,
    registry: prometheus::Registry,
}

impl TelemetryService for PrometheusTelemetryService {
    fn new(config: TelemetryConfig) -> Arc<Self>
    where
        Self: Sized,
    {
        Arc::new(Self {
            config,
            registry: prometheus::Registry::new(),
        })
    }

    fn register_f64_counter(
        &self,
        name: &str,
        help: &str,
        labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<f64>>, TelemetryError> {
        let const_labels = self
            .const_labels()
            .into_iter()
            .chain(labels.unwrap_or_default())
            .collect::<HashMap<_, _>>();
        let opts = prometheus::Opts::new(name, help).const_labels(const_labels);
        let counter = prometheus::Counter::with_opts(opts).map_err(|e| {
            TelemetryError::RegisterCounterError(format!("Failed to create counter: {e}"))
        })?;
        self.registry
            .register(Box::new(counter.clone()))
            .map_err(|e| {
                TelemetryError::RegisterCounterError(format!("Failed to register counter: {e}"))
            })?;
        Ok(Arc::new(PrometheusCounter { inner: counter }))
    }

    fn register_u64_counter(
        &self,
        name: &str,
        help: &str,
        labels: Option<HashMap<String, String>>,
    ) -> Result<Arc<dyn Counter<u64>>, TelemetryError> {
        let const_labels = self
            .const_labels()
            .into_iter()
            .chain(labels.unwrap_or_default())
            .collect::<HashMap<_, _>>();
        let opts = prometheus::Opts::new(name, help).const_labels(const_labels);
        let counter = prometheus::IntCounter::with_opts(opts).map_err(|e| {
            TelemetryError::RegisterCounterError(format!("Failed to create counter: {e}"))
        })?;
        self.registry
            .register(Box::new(counter.clone()))
            .map_err(|e| {
                TelemetryError::RegisterCounterError(format!("Failed to register counter: {e}"))
            })?;
        Ok(Arc::new(PrometheusCounter { inner: counter }))
    }

    fn register_u64_gauge_vec(
        &self,
        name: &str,
        help: &str,
        label_names: &[&str],
    ) -> Result<Arc<dyn GaugeVec<u64>>, TelemetryError> {
        let const_labels = self.const_labels();
        let opts = prometheus::Opts::new(name, help).const_labels(const_labels);
        let vec = prometheus::IntGaugeVec::new(opts, label_names).map_err(|e| {
            TelemetryError::RegisterGaugeError(format!("Failed to create gauge vec: {e}"))
        })?;
        self.registry.register(Box::new(vec.clone())).map_err(|e| {
            TelemetryError::RegisterGaugeError(format!("Failed to register gauge vec: {e}"))
        })?;
        Ok(Arc::new(PrometheusIntGaugeVec {
            inner: vec,
            metric_name: name.to_string(),
            label_arity: label_names.len(),
        }))
    }

    fn get_feed(&self) -> Result<String, TelemetryError> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder.encode_to_string(&metric_families).map_err(|e| {
            TelemetryError::EncodeMetricsError(format!("Failed to encode metrics: {e}"))
        })
    }

    fn service_name(&self) -> String {
        self.config.service_name.clone()
    }

    fn ip_address(&self) -> String {
        self.config.ip_address.clone()
    }
}

impl PrometheusTelemetryService {
    fn const_labels(&self) -> HashMap<String, String> {
        HashMap::from([
            ("service".to_string(), self.config.service_name.clone()),
            ("ip_address".to_string(), self.config.ip_address.clone()),
        ])
    }
}

struct PrometheusCounter<T: Atomic> {
    inner: prometheus::core::GenericCounter<T>,
}

impl Counter<f64> for PrometheusCounter<AtomicF64> {
    fn inc(&self) {
        self.inner.inc();
    }
    fn inc_by(&self, value: f64) {
        self.inner.inc_by(value);
    }
}

impl Counter<u64> for PrometheusCounter<AtomicU64> {
    fn inc(&self) {
        self.inner.inc();
    }
    fn inc_by(&self, value: u64) {
        self.inner.inc_by(value);
    }
}

struct PrometheusIntGaugeVec {
    inner: prometheus::IntGaugeVec,
    metric_name: String,
    label_arity: usize,
}

impl GaugeVec<u64> for PrometheusIntGaugeVec {
    fn set(&self, label_values: &[&str], value: u64) {
        if label_values.len() != self.label_arity {
            warn!(
                metric = %self.metric_name,
                expected = self.label_arity,
                got = label_values.len(),
                "GaugeVec::set called with wrong label arity; skipping update"
            );
            return;
        }
        // The prometheus crate signs `IntGauge`'s value as i64
        // internally even when the API takes a u64 atomically. Saturate
        // on overflow rather than wrap — a value past `i64::MAX` here
        // means something has gone badly wrong with the producer, and
        // reporting saturation is the least-bad observable state.
        let signed = i64::try_from(value).unwrap_or(i64::MAX);
        match self.inner.get_metric_with_label_values(label_values) {
            Ok(child) => child.set(signed),
            Err(e) => warn!(
                metric = %self.metric_name,
                error = %e,
                "GaugeVec::set could not resolve label-values child; skipping update"
            ),
        }
    }

    fn remove_label_values(&self, label_values: &[&str]) {
        if label_values.len() != self.label_arity {
            warn!(
                metric = %self.metric_name,
                expected = self.label_arity,
                got = label_values.len(),
                "GaugeVec::remove_label_values called with wrong label arity; skipping"
            );
            return;
        }
        if let Err(e) = self.inner.remove_label_values(label_values) {
            warn!(
                metric = %self.metric_name,
                error = %e,
                "GaugeVec::remove_label_values failed; skipping"
            );
        }
    }

    fn reset(&self) {
        self.inner.reset();
    }
}
