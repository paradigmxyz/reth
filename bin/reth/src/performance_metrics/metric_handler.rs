use super::metric_storage::{MetricsStorage, PerformanceDashboardMetricStorage};
use std::sync::Arc;

use revm_utils::time::*;
use std::{
    ops::{Div, Mul},
    time::Duration,
};
use tokio::time::sleep;
use tracing::*;

pub(crate) struct PerformanceDashboardMetricHandler {
    storage: Arc<PerformanceDashboardMetricStorage>,
    pre_snapshot: Option<MetricsStorage>,
    cnt: u64,
    cpu_frequency: f64,
}

impl PerformanceDashboardMetricHandler {
    pub(crate) fn new(storage: Arc<PerformanceDashboardMetricStorage>) -> Self {
        Self {
            storage,
            cnt: 0,
            pre_snapshot: None,
            cpu_frequency: get_cpu_frequency().expect("Get cpu frequency error!"),
        }
    }

    pub(crate) async fn run(&mut self, interval: u64) {
        let mut duration_record_counter = TimeRecorder::now();
        loop {
            {
                let snapshot = self.storage.snapshot();
                let elapsed_ns =
                    duration_record_counter.record_next_time().to_nanoseconds(self.cpu_frequency);

                if let Some(pre_snapshot) = self.pre_snapshot.take() {
                    // 1. calculate tps
                    let delta_txs = snapshot
                        .total_txs_processed
                        .checked_sub(pre_snapshot.total_txs_processed)
                        .expect("overflow");
                    let tps = delta_txs.mul(1000_000_000).div(elapsed_ns);

                    // 2. calculate mGas/s
                    let delta_mgas = snapshot.total_mgas_used - pre_snapshot.total_mgas_used;
                    let mgas_ps = delta_mgas.mul(1000_000_000 as f64).div(elapsed_ns as f64);

                    info!(target: "performance_dashboard_metrics.sync_stage.execution", "tps =====> {:?}", tps);
                    info!(target: "performance_dashboard_metrics.sync_stage.execution", "mGas/s =====> {:?}", mgas_ps);
                    self.cnt += 1;

                    // 3. total
                    let execute_inner_time = snapshot.execute_inner_time;
                    if execute_inner_time != 0 {
                        info!(target: "performance_dashboard_metrics.sync_stage.execution", "execute inner time =====> {:?}", execute_inner_time);
                    }
                    let read_block_info_time = snapshot.read_block_info_time;
                    if read_block_info_time != 0 {
                        info!(target: "performance_dashboard_metrics.sync_stage.execution", "total read block info time =====> {:?}", read_block_info_time);
                    }
                    let revm_execute_time = snapshot.revm_execute_time;
                    if revm_execute_time != 0 {
                        info!(target: "performance_dashboard_metrics.sync_stage.execution", "total revm execute time =====> {:?}", revm_execute_time);
                    }
                    let post_process_time = snapshot.post_process_time;
                    if post_process_time != 0 {
                        info!(target: "performance_dashboard_metrics.sync_stage.execution", "total post process time =====> {:?}", post_process_time);
                    }
                    let write_to_db_time = snapshot.write_to_db_time;
                    if write_to_db_time != 0 {
                        info!(target: "performance_dashboard_metrics.sync_stage.execution", "total write to db time =====> {:?}", write_to_db_time);
                    }
                }

                self.record(snapshot);
            }
            sleep(Duration::from_secs(interval)).await;
        }
    }

    fn record(&mut self, snapshot: MetricsStorage) {
        self.pre_snapshot = Some(snapshot);
    }
}
