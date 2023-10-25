#[cfg(feature = "open_performance_dashboard")]
use reth_metrics::metrics::Counter;
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives::stage::StageId;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct SyncMetrics {
    pub(crate) stages: HashMap<StageId, StageMetrics>,
    pub(crate) execution_stage: ExecutionStageMetrics,
}

impl SyncMetrics {
    /// Returns existing or initializes a new instance of [StageMetrics] for the provided [StageId].
    pub(crate) fn get_stage_metrics(&mut self, stage_id: StageId) -> &mut StageMetrics {
        self.stages
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
    }
}

#[derive(Metrics)]
#[metrics(scope = "sync")]
pub(crate) struct StageMetrics {
    /// The block number of the last commit for a stage.
    pub(crate) checkpoint: Gauge,
    /// The number of processed entities of the last commit for a stage, if applicable.
    pub(crate) entities_processed: Gauge,
    /// The number of total entities of the last commit for a stage, if applicable.
    pub(crate) entities_total: Gauge,
}

/// Execution stage metrics.
#[derive(Metrics)]
#[metrics(scope = "sync.execution")]
pub(crate) struct ExecutionStageMetrics {
    /// The total amount of gas processed (in millions).
    pub(crate) mgas_processed_total: Gauge,
    /// The total amount of transactions processed.
    #[cfg(feature = "enable_tps_gas_record")]
    pub(crate) txs_processed_total: Counter,
    /// Time of execute inner.
    #[cfg(feature = "enable_execution_duration_record")]
    pub(crate) execute_inner_time: Counter,
    /// Time of read block info.
    #[cfg(feature = "enable_execution_duration_record")]
    pub(crate) read_block_info_time: Counter,
    /// Time of revm execute tx.
    #[cfg(feature = "enable_execution_duration_record")]
    pub(crate) revm_execute_time: Counter,
    /// Post process time.
    #[cfg(feature = "enable_execution_duration_record")]
    pub(crate) post_process_time: Counter,
    /// Time of write to db.
    #[cfg(feature = "enable_execution_duration_record")]
    pub(crate) write_to_db_time: Counter,

    /// total time of read header td from db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) read_header_td_db_time: Counter,
    /// total data size of read header td from db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) read_header_td_db_size: Counter,
    /// total time of read block with senders from db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) read_block_with_senders_db_time: Counter,
    /// total data size of read block with senders from db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) read_block_with_senders_db_size: Counter,
    /// time of write to db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) db_speed_write_to_db_time: Counter,
    /// data size of write to db
    #[cfg(feature = "enable_db_speed_record")]
    pub(crate) db_speed_write_to_db_size: Counter,
}
