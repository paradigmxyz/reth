use crate::metrics::SyncMetrics;
use reth_primitives::{
    constants::MGAS_TO_GAS,
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::trace;

#[cfg(feature = "enable_db_speed_record")]
use super::util::DbSpeedRecord;
#[cfg(feature = "enable_execution_duration_record")]
use super::util::ExecutionDurationRecord;

#[cfg(feature = "enable_cache_record")]
use revm_utils::types::*;

/// Alias type for metric producers to use.
pub type MetricEventsSender = UnboundedSender<MetricEvent>;

/// Collection of metric events.
#[derive(Clone, Copy, Debug)]
pub enum MetricEvent {
    /// Sync reached new height. All stage checkpoints are updated.
    SyncHeight {
        /// Maximum height measured in block number that sync reached.
        height: BlockNumber,
    },
    /// Stage reached new checkpoint.
    StageCheckpoint {
        /// Stage ID.
        stage_id: StageId,
        /// Stage checkpoint.
        checkpoint: StageCheckpoint,
        /// Maximum known block number reachable by this stage.
        /// If specified, `entities_total` metric is updated.
        max_block_number: Option<BlockNumber>,
    },
    /// Execution stage processed some amount of gas.
    ExecutionStageGas {
        /// Gas processed.
        gas: u64,
    },
    /// Execution stage processed .
    #[cfg(feature = "enable_execution_duration_record")]
    ExecutionStageTime {
        /// excution duration record.
        execute_duration_record: ExecutionDurationRecord,
    },
    /// Execution stage processed some amount of txs and gas in a block.
    #[cfg(feature = "enable_tps_gas_record")]
    BlockTpsAndGas {
        /// block number
        block_number: u64,
        /// Txs processed.
        txs: u64,
        /// Gas processed.
        gas: u64,
    },
    /// block tps and gas record switch
    #[cfg(feature = "enable_tps_gas_record")]
    BlockTpsAndGasSwitch {
        /// true:start block tps and gas record.
        /// false: stop block tps and gas record.
        switch: bool,
    },
    /// db speed metric record
    #[cfg(feature = "enable_db_speed_record")]
    DBSpeedInfo {
        /// db speed record.
        db_speed_record: DbSpeedRecord,
    },
    /// opcode record in revm
    #[cfg(feature = "enable_opcode_metrics")]
    RevmMetricTime {
        /// opcode record in revm.
        revm_metric_record: revm_utils::types::RevmMetricRecord,
    },
    /// cacheDB size when execute a block
    #[cfg(feature = "enable_cache_record")]
    CacheDbSizeInfo {
        /// block number
        block_number: u64,
        /// cache db size
        cachedb_size: usize,
    },
    /// revm cacheDB metric record
    #[cfg(feature = "enable_cache_record")]
    CacheDbInfo {
        /// cache db record.
        cache_db_record: CacheDbRecord,
    },
}

/// Metrics routine that listens to new metric events on the `events_rx` receiver.
/// Upon receiving new event, related metrics are updated.
#[derive(Debug)]
pub struct MetricsListener {
    events_rx: UnboundedReceiver<MetricEvent>,
    pub(crate) sync_metrics: SyncMetrics,

    #[cfg(feature = "open_performance_dashboard")]
    dashboard_events_tx: MetricEventsSender,
}

impl MetricsListener {
    #[cfg(not(feature = "open_performance_dashboard"))]
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_rx, sync_metrics: SyncMetrics::default() }
    }

    #[cfg(feature = "open_performance_dashboard")]
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>, events_tx: MetricEventsSender) -> Self {
        Self { events_rx, sync_metrics: SyncMetrics::default(), dashboard_events_tx: events_tx }
    }

    fn handle_event(&mut self, event: MetricEvent) {
        trace!(target: "sync::metrics", ?event, "Metric event received");
        match event {
            MetricEvent::SyncHeight { height } => {
                for stage_id in StageId::ALL {
                    self.handle_event(MetricEvent::StageCheckpoint {
                        stage_id,
                        checkpoint: StageCheckpoint {
                            block_number: height,
                            stage_checkpoint: None,
                        },
                        max_block_number: Some(height),
                    });
                }
            }
            MetricEvent::StageCheckpoint { stage_id, checkpoint, max_block_number } => {
                let stage_metrics = self.sync_metrics.get_stage_metrics(stage_id);

                stage_metrics.checkpoint.set(checkpoint.block_number as f64);

                let (processed, total) = match checkpoint.entities() {
                    Some(entities) => (entities.processed, Some(entities.total)),
                    None => (checkpoint.block_number, max_block_number),
                };

                stage_metrics.entities_processed.set(processed as f64);

                if let Some(total) = total {
                    stage_metrics.entities_total.set(total as f64);
                }
            }
            MetricEvent::ExecutionStageGas { gas } => self
                .sync_metrics
                .execution_stage
                .mgas_processed_total
                .increment(gas as f64 / MGAS_TO_GAS as f64),
            #[cfg(feature = "enable_execution_duration_record")]
            MetricEvent::ExecutionStageTime { execute_duration_record } => {
                let _ = self
                    .dashboard_events_tx
                    .send(MetricEvent::ExecutionStageTime { execute_duration_record });
            }
            #[cfg(feature = "enable_tps_gas_record")]
            MetricEvent::BlockTpsAndGas { block_number, txs, gas } => {
                let _ = self.dashboard_events_tx.send(MetricEvent::BlockTpsAndGas {
                    block_number,
                    txs,
                    gas,
                });
            }
            #[cfg(feature = "enable_tps_gas_record")]
            MetricEvent::BlockTpsAndGasSwitch { switch } => {
                let _ = self.dashboard_events_tx.send(MetricEvent::BlockTpsAndGasSwitch { switch });
            }
            #[cfg(feature = "enable_db_speed_record")]
            MetricEvent::DBSpeedInfo { db_speed_record } => {
                let _ = self.dashboard_events_tx.send(MetricEvent::DBSpeedInfo { db_speed_record });
            }
            #[cfg(feature = "enable_opcode_metrics")]
            MetricEvent::RevmMetricTime { revm_metric_record } => {
                let _ = self
                    .dashboard_events_tx
                    .send(MetricEvent::RevmMetricTime { revm_metric_record });
            }
            #[cfg(feature = "enable_cache_record")]
            MetricEvent::CacheDbSizeInfo { block_number, cachedb_size } => {
                let _ = self
                    .dashboard_events_tx
                    .send(MetricEvent::CacheDbSizeInfo { block_number, cachedb_size });
            }
            #[cfg(feature = "enable_cache_record")]
            MetricEvent::CacheDbInfo { cache_db_record } => {
                let _ = self.dashboard_events_tx.send(MetricEvent::CacheDbInfo { cache_db_record });
            }
        }
    }
}

impl Future for MetricsListener {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Loop until we drain the `events_rx` channel
        loop {
            let Some(event) = ready!(this.events_rx.poll_recv(cx)) else {
                // Channel has closed
                return Poll::Ready(())
            };

            this.handle_event(event);
        }
    }
}
