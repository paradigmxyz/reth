use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub(crate) struct PerformanceDashboardMetricStorage {
    pub(crate) total_mgas_used: Arc<Mutex<f64>>,
    pub(crate) total_txs_processed: Arc<Mutex<u64>>,
    pub(crate) execute_inner_time: Arc<Mutex<u64>>,
    pub(crate) read_block_info_time: Arc<Mutex<u64>>,
    pub(crate) revm_execute_time: Arc<Mutex<u64>>,
    pub(crate) post_process_time: Arc<Mutex<u64>>,
    pub(crate) write_to_db_time: Arc<Mutex<u64>>,
}
impl PerformanceDashboardMetricStorage {
    pub(crate) fn snapshot(&self) -> MetricsStorage {
        MetricsStorage::from(self)
    }
}

#[derive(Debug)]
pub(crate) struct MetricsStorage {
    pub(crate) total_mgas_used: f64,
    pub(crate) total_txs_processed: u64,
    pub(crate) execute_inner_time: u64,
    pub(crate) read_block_info_time: u64,
    pub(crate) revm_execute_time: u64,
    pub(crate) post_process_time: u64,
    pub(crate) write_to_db_time: u64,
}

impl From<&PerformanceDashboardMetricStorage> for MetricsStorage {
    fn from(storage: &PerformanceDashboardMetricStorage) -> Self {
        let total_mgas_used = {
            let guard = storage.total_mgas_used.lock();
            *guard
        };

        let total_txs_processed = {
            let guard = storage.total_txs_processed.lock();
            *guard
        };

        let execute_inner_time = {
            let guard = storage.execute_inner_time.lock();
            *guard
        };

        let read_block_info_time = {
            let guard = storage.read_block_info_time.lock();
            *guard
        };

        let revm_execute_time = {
            let guard = storage.revm_execute_time.lock();
            *guard
        };

        let post_process_time = {
            let guard = storage.post_process_time.lock();
            *guard
        };

        let write_to_db_time = {
            let guard = storage.write_to_db_time.lock();
            *guard
        };

        Self {
            total_mgas_used,
            total_txs_processed,
            execute_inner_time,
            read_block_info_time,
            revm_execute_time,
            post_process_time,
            write_to_db_time,
        }
    }
}
