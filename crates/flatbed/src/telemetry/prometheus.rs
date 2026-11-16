//! [`TelemetryService`] implementation backed by a
//! [`prometheus::Registry`], available via the `prometheus` feature
//! flag. Supports counters (`f64`/`u64`), labeled counters (`u64`) and
//! labeled gauges (`u64`).

use std::collections::HashMap;
use std::sync::Arc;

use crate::telemetry::{
    Counter, CounterVec, GaugeVec, TelemetryConfig, TelemetryError, TelemetryService,
};
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

    fn register_u64_counter_vec(
        &self,
        name: &str,
        help: &str,
        label_names: &[&str],
    ) -> Result<Arc<dyn CounterVec<u64>>, TelemetryError> {
        let opts = prometheus::Opts::new(name, help).const_labels(self.const_labels());
        let vec = prometheus::IntCounterVec::new(opts, label_names).map_err(|e| {
            TelemetryError::RegisterCounterError(format!("Failed to create counter vec: {e}"))
        })?;
        self.registry.register(Box::new(vec.clone())).map_err(|e| {
            TelemetryError::RegisterCounterError(format!("Failed to register counter vec: {e}"))
        })?;
        Ok(Arc::new(PrometheusIntCounterVec {
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

struct PrometheusIntCounterVec {
    inner: prometheus::IntCounterVec,
    metric_name: String,
    label_arity: usize,
}

impl CounterVec<u64> for PrometheusIntCounterVec {
    fn inc_by(&self, label_values: &[&str], value: u64) {
        if label_values.len() != self.label_arity {
            warn!(
                metric = %self.metric_name,
                expected = self.label_arity,
                got = label_values.len(),
                "CounterVec::inc_by called with wrong label arity; skipping update"
            );
            return;
        }
        match self.inner.get_metric_with_label_values(label_values) {
            Ok(child) => child.inc_by(value),
            Err(e) => warn!(
                metric = %self.metric_name,
                error = %e,
                "CounterVec::inc_by could not resolve label-values child; skipping update"
            ),
        }
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

#[cfg(test)]
mod tests {
    use super::PrometheusTelemetryService;
    use crate::telemetry::{TelemetryConfig, TelemetryService};

    #[test]
    fn a_counter_vec_feeds_one_series_per_label_tuple() {
        let telemetry = PrometheusTelemetryService::new(TelemetryConfig {
            service_name: "diadem".to_string(),
            ip_address: "10.0.0.1".to_string(),
            port: 9090,
        });
        let requests = telemetry
            .register_u64_counter_vec("requests_total", "Requests.", &["namespace", "alias"])
            .unwrap();

        requests.inc_by(&["acme", "fast"], 2);
        requests.inc_by(&["acme", "fast"], 1);
        requests.inc_by(&["globex", "smart"], 5);
        requests.inc_by(&["missing-alias"], 9);

        let feed = telemetry.get_feed().unwrap();
        let series: Vec<&str> = feed
            .lines()
            .filter(|line| line.starts_with("requests_total{"))
            .collect();
        assert_eq!(series.len(), 2, "{feed}");
        assert!(
            series
                .iter()
                .any(|line| line.contains(r#"namespace="acme""#)
                    && line.contains(r#"alias="fast""#)
                    && line.contains(r#"service="diadem""#)
                    && line.ends_with(" 3")),
            "{feed}"
        );
        assert!(
            series
                .iter()
                .any(|line| line.contains(r#"namespace="globex""#)
                    && line.contains(r#"alias="smart""#)
                    && line.ends_with(" 5")),
            "{feed}"
        );
    }
}
