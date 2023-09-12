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

// #[cfg(feature = "open_revm_metrics_record")]
// use revm_utils::types::RevmMetricRecord;

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
    /// Execution stage processed some amount of txs.
    #[cfg(feature = "open_performance_dashboard")]
    ExecutionStageTxs {
        /// Txs processed.
        txs: u64,
    },
    // /// Revm metric record.
    // #[cfg(feature = "open_revm_metrics_record")]
    // RevmMetricRecord {
    //     /// Revm metric record.
    //     record: RevmMetricRecord,
    //     /// size of cacheDb.
    //     cachedb_size: usize,
    // },
    /// Time of get block info.
    #[cfg(feature = "open_performance_dashboard")]
    ReadBlockInfoTime {
        /// time.
        time: u64,
    },
    /// Time of revm execute tx.
    #[cfg(feature = "open_performance_dashboard")]
    RevmExecuteTxTime {
        /// time.
        time: u64,
    },
    /// Post process time.
    #[cfg(feature = "open_performance_dashboard")]
    PostProcessTime {
        /// time.
        time: u64,
    },
    /// Time of write to db.
    #[cfg(feature = "open_performance_dashboard")]
    WriteToDbTime {
        /// time.
        time: u64,
    },
}

/// Metrics routine that listens to new metric events on the `events_rx` receiver.
/// Upon receiving new event, related metrics are updated.
#[derive(Debug)]
pub struct MetricsListener {
    events_rx: UnboundedReceiver<MetricEvent>,
    pub(crate) sync_metrics: SyncMetrics,
}

impl MetricsListener {
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_rx, sync_metrics: SyncMetrics::default() }
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
            #[cfg(feature = "open_performance_dashboard")]
            MetricEvent::ExecutionStageTxs { txs } => {
                self.sync_metrics.execution_stage.txs_processed_total.increment(txs)
            }
            #[cfg(feature = "open_performance_dashboard")]
            MetricEvent::ReadBlockInfoTime { time } => {
                self.sync_metrics.execution_stage.read_block_info_time.increment(time)
            }
            #[cfg(feature = "open_performance_dashboard")]
            MetricEvent::RevmExecuteTxTime { time } => {
                self.sync_metrics.execution_stage.revm_execute_time.increment(time)
            }
            #[cfg(feature = "open_performance_dashboard")]
            MetricEvent::PostProcessTime { time } => {
                self.sync_metrics.execution_stage.post_process_time.increment(time)
            }
            #[cfg(feature = "open_performance_dashboard")]
            MetricEvent::WriteToDbTime { time } => {
                self.sync_metrics.execution_stage.write_to_db_time.increment(time)
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
