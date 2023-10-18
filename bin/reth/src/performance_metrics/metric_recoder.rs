use super::metric_storage::PerformanceDashboardMetricStorage;
use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Recorder,
    SharedString, Unit,
};
use std::sync::Arc;
use tracing::*;

struct Handle {
    key: Key,
    storage: Arc<PerformanceDashboardMetricStorage>,
}

impl<'a> Handle {
    fn new(key: Key, storage: Arc<PerformanceDashboardMetricStorage>) -> Handle {
        Handle { key, storage }
    }
}

impl CounterFn for Handle {
    fn increment(&self, value: u64) {
        match self.key.name() {
            "sync.execution.txs_processed_total" => {
                let mut guard = self.storage.total_txs_processed.lock();
                *guard = (*guard).checked_add(value).expect("counter txs_processed_total overflow");
            }
            "sync.execution.execute_inner_time" => {
                let mut guard = self.storage.execute_inner_time.lock();
                *guard =
                    (*guard).checked_add(value).expect("counter execute_inner_time overflow");
            }
            "sync.execution.read_block_info_time" => {
                let mut guard = self.storage.read_block_info_time.lock();
                *guard =
                    (*guard).checked_add(value).expect("counter read_block_info_time overflow");
            }
            "sync.execution.revm_execute_time" => {
                let mut guard = self.storage.revm_execute_time.lock();
                *guard = (*guard).checked_add(value).expect("counter revm_execute_time overflow");
            }
            "sync.execution.post_process_time" => {
                let mut guard = self.storage.post_process_time.lock();
                *guard = (*guard).checked_add(value).expect("counter post_process_time overflow");
            }
            "sync.execution.write_to_db_time" => {
                let mut guard = self.storage.write_to_db_time.lock();
                *guard = (*guard).checked_add(value).expect("counter write_to_db_time overflow");
            }
            _ => {}
        }
    }

    fn absolute(&self, _value: u64) {}
}

impl GaugeFn for Handle {
    fn increment(&self, value: f64) {
        match self.key.name() {
            "sync.execution.mgas_processed_total" => {
                let mut guard = self.storage.total_mgas_used.lock();
                *guard += value;
            }
            _ => {}
        }
    }

    fn decrement(&self, _value: f64) {}
    fn set(&self, _value: f64) {}
}

impl HistogramFn for Handle {
    fn record(&self, _value: f64) {}
}

pub(crate) struct PerformanceDashboardRecorder {
    storage: Arc<PerformanceDashboardMetricStorage>,
}
impl PerformanceDashboardRecorder {
    pub(crate) fn new(storage: Arc<PerformanceDashboardMetricStorage>) -> Self {
        Self { storage }
    }
}

impl Recorder for PerformanceDashboardRecorder {
    fn describe_counter(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
        info!(target: "performance_dashboard_metrics",
            "(counter) registered key {} with unit {:?} and description {:?}",
            key_name.as_str(),
            unit,
            description
        );
    }

    fn describe_gauge(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
        info!(target: "performance_dashboard_metrics",
            "(gauge) registered key {} with unit {:?} and description {:?}",
            key_name.as_str(),
            unit,
            description
        );
    }

    fn describe_histogram(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
        info!(target: "performance_dashboard_metrics",
            "(histogram) registered key {} with unit {:?} and description {:?}",
            key_name.as_str(),
            unit,
            description
        );
    }

    fn register_counter(&self, key: &Key) -> Counter {
        Counter::from_arc(Arc::new(Handle::new(key.clone(), self.storage.clone())))
    }

    fn register_gauge(&self, key: &Key) -> Gauge {
        Gauge::from_arc(Arc::new(Handle::new(key.clone(), self.storage.clone())))
    }

    fn register_histogram(&self, key: &Key) -> Histogram {
        Histogram::from_arc(Arc::new(Handle::new(key.clone(), self.storage.clone())))
    }
}
