use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

use tokio::sync::mpsc::UnboundedReceiver;
use tracing::trace;

use reth_stages::MetricEvent;

#[cfg(feature = "enable_opcode_metrics")]
use super::dashboard_display::RevmMetricTimeDisplayer;

#[cfg(feature = "enable_execution_duration_record")]
use super::dashboard_display::ExecutionDurationDisplayer;

#[cfg(feature = "enable_db_speed_record")]
use super::dashboard_display::DBSpeedDisplayer;

#[cfg(feature = "enable_cache_record")]
use super::dashboard_display::CacheDBRecordDisplayer;

#[cfg(feature = "enable_tps_gas_record")]
use super::dashboard_display::TpsAndGasRecordDisplayer;

#[derive(Debug)]
pub(crate) struct DashboardListener {
    events_rx: UnboundedReceiver<MetricEvent>,

    #[cfg(feature = "enable_opcode_metrics")]
    revm_metric_displayer: RevmMetricTimeDisplayer,

    #[cfg(feature = "enable_execution_duration_record")]
    excution_dureation_displayer: ExecutionDurationDisplayer,

    #[cfg(feature = "enable_db_speed_record")]
    db_speed_displayer: DBSpeedDisplayer,

    #[cfg(feature = "enable_cache_record")]
    cache_db_displayer: CacheDBRecordDisplayer,

    #[cfg(feature = "enable_tps_gas_record")]
    tps_gas_displayer: TpsAndGasRecordDisplayer,
}

impl DashboardListener {
    /// Creates a new [DashboardListener] with the provided receiver of [MetricEvent].
    pub(crate) fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self {
            events_rx,

            #[cfg(feature = "enable_opcode_metrics")]
            revm_metric_displayer: RevmMetricTimeDisplayer::default(),

            #[cfg(feature = "enable_execution_duration_record")]
            excution_dureation_displayer: ExecutionDurationDisplayer::default(),

            #[cfg(feature = "enable_db_speed_record")]
            db_speed_displayer: DBSpeedDisplayer::default(),

            #[cfg(feature = "enable_cache_record")]
            cache_db_displayer: CacheDBRecordDisplayer::default(),

            #[cfg(feature = "enable_tps_gas_record")]
            tps_gas_displayer: TpsAndGasRecordDisplayer::default(),
        }
    }

    fn handle_event(&mut self, event: MetricEvent) {
        trace!(target: "sync::dashboard", ?event, "Dashboard event received");
        match event {
            MetricEvent::SyncHeight { .. } => {}
            MetricEvent::StageCheckpoint { .. } => {}
            MetricEvent::ExecutionStageGas { .. } => {}
            #[cfg(feature = "enable_execution_duration_record")]
            MetricEvent::ExecutionStageTime { execute_duration_record } => {
                self.excution_dureation_displayer
                    .update_excution_duration_record(execute_duration_record);
                self.excution_dureation_displayer.print();
            }
            #[cfg(feature = "enable_tps_gas_record")]
            MetricEvent::BlockTpsAndGas { block_number, txs, gas } => {
                self.tps_gas_displayer.update_tps_and_gas(block_number, txs, gas);
            }
            #[cfg(feature = "enable_tps_gas_record")]
            MetricEvent::BlockTpsAndGasSwitch { switch } => {
                if switch {
                    self.tps_gas_displayer.start_record();
                } else {
                    self.tps_gas_displayer.stop_record();
                }
            }
            #[cfg(feature = "enable_db_speed_record")]
            MetricEvent::DBSpeedInfo { db_speed_record } => {
                self.db_speed_displayer.update_db_speed_record(db_speed_record);
                self.db_speed_displayer.print();
            }
            #[cfg(feature = "enable_opcode_metrics")]
            MetricEvent::RevmMetricTime { mut revm_metric_record } => {
                self.revm_metric_displayer.update_metric_record(&mut revm_metric_record);
                self.revm_metric_displayer.print();
            }
            #[cfg(feature = "enable_cache_record")]
            MetricEvent::CacheDbSizeInfo { block_number, cachedb_size } => {
                self.cache_db_displayer.update_size(block_number, cachedb_size);
            }
            #[cfg(feature = "enable_cache_record")]
            MetricEvent::CacheDbInfo { cache_db_record } => {
                self.cache_db_displayer.update_cache_db_record(cache_db_record);
                self.cache_db_displayer.print();
            }
        }
    }
}

impl Future for DashboardListener {
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
