use crate::metrics::SyncMetrics;
#[cfg(feature = "enable_tps_gas_record")]
use reth_primitives::constants::MGAS_TO_GAS;
use reth_primitives::{
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
    // /// Revm metric record.
    // #[cfg(feature = "enable_opcode_metrics")]
    // RevmMetricRecord {
    //     /// Revm metric record.
    //     record: RevmMetricRecord,
    //     /// size of cacheDb.
    //     cachedb_size: usize,
    // },
    /// Execution stage processed .
    #[cfg(feature = "enable_execution_duration_record")]
    ExecutionStageTime {
        /// total time of execute_inner
        execute_inner: u128,
        /// total time of  get block td and block_with_senders
        read_block: u128,
        /// time of revm execute tx(execute_and_verify_receipt)
        execute_tx: u128,
        /// time of process state(state.extend)
        process_state: u128,
        /// time of write to db
        write_to_db: u128,
    },
    /// Execution stage processed some amount of txs and gas in a block.
    #[cfg(feature = "enable_tps_gas_record")]
    BlockTpsAndGas {
        /// Txs processed.
        txs: u64,
        /// Gas processed.
        gas: u64,
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

            #[cfg(feature = "enable_execution_duration_record")]
            MetricEvent::ExecutionStageTime {
                execute_inner,
                read_block,
                execute_tx,
                process_state,
                write_to_db,
            } => {
                self.sync_metrics
                    .execution_stage
                    .execute_inner_time
                    .increment(execute_inner.try_into().expect("truncation error"));
                self.sync_metrics
                    .execution_stage
                    .read_block_info_time
                    .increment(read_block.try_into().expect("truncation error"));
                self.sync_metrics
                    .execution_stage
                    .revm_execute_time
                    .increment(execute_tx.try_into().expect("truncation error"));
                self.sync_metrics
                    .execution_stage
                    .post_process_time
                    .increment(process_state.try_into().expect("truncation error"));
                self.sync_metrics
                    .execution_stage
                    .write_to_db_time
                    .increment(write_to_db.try_into().expect("truncation error"));
            }
            #[cfg(feature = "enable_tps_gas_record")]
            MetricEvent::BlockTpsAndGas { txs, gas } => {
                self.sync_metrics.execution_stage.txs_processed_total.increment(txs);
                self.sync_metrics
                    .execution_stage
                    .mgas_processed_total
                    .increment(gas as f64 / MGAS_TO_GAS as f64);
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
