//! Transaction pool orderflow monitoring.
//!
//! Monitors the share of public orderflow (transactions) that the node had locally available in
//! the pool when a block was mined. This is useful for assessing mempool health and understanding
//! how well-connected the node is.
//!
//! Two snapshots are taken per slot:
//! - A mid-slot snapshot taken at a configurable offset (default: 4s) after the last canonical
//!   block, capturing the pool state during the block building window.
//! - An on-block snapshot taken when a new canonical block notification is received, capturing the
//!   pool state at the moment the block is observed.

use crate::traits::TransactionPool;
use alloy_consensus::BlockHeader;
use alloy_primitives::{map::HashSet, BlockNumber, TxHash};
use futures_util::{FutureExt, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::NodePrimitives;
use std::time::Duration;
use tokio::time::Sleep;
use tracing::{debug, info};

/// Default offset into the slot at which the mid-slot snapshot is taken.
///
/// 4 seconds matches the Ethereum block building window even though the slot time is 12s.
pub const DEFAULT_MONITOR_SLOT_OFFSET: Duration = Duration::from_secs(4);

/// Metrics for the transaction pool monitor.
#[derive(Metrics)]
#[metrics(scope = "transaction_pool.orderflow")]
pub struct OrderflowMonitorMetrics {
    /// Total number of transactions in the mined block (mid-slot observation).
    pub mid_slot_mined_transactions: Gauge,
    /// Number of mined transactions that were present in the local pool at the mid-slot snapshot.
    pub mid_slot_matched_transactions: Gauge,
    /// Ratio of matched/mined transactions at mid-slot (0.0 to 1.0 scaled to 0-10000 for gauge
    /// precision).
    pub mid_slot_match_ratio_bps: Gauge,

    /// Total number of transactions in the mined block (on-block observation).
    pub on_block_mined_transactions: Gauge,
    /// Number of mined transactions that were present in the local pool at the on-block snapshot.
    pub on_block_matched_transactions: Gauge,
    /// Ratio of matched/mined transactions at on-block (0.0 to 1.0 scaled to 0-10000 for gauge
    /// precision).
    pub on_block_match_ratio_bps: Gauge,

    /// Histogram of mid-slot match ratios.
    pub mid_slot_match_ratio: Histogram,
    /// Histogram of on-block match ratios.
    pub on_block_match_ratio: Histogram,

    /// Counter of total blocks monitored.
    pub blocks_monitored: Counter,
}

/// A snapshot of transaction hashes currently in the pool.
#[derive(Debug)]
struct PoolSnapshot {
    /// The block number after which this snapshot was taken (i.e. the last canonical block
    /// number).
    after_block: BlockNumber,
    /// Transaction hashes of all pending (executable) transactions at snapshot time.
    pending_hashes: HashSet<TxHash>,
}

/// Configuration for the pool monitor.
#[derive(Debug, Clone)]
pub struct OrderflowMonitorConfig {
    /// Offset into the slot at which the mid-slot snapshot is taken.
    pub slot_offset: Duration,
}

impl Default for OrderflowMonitorConfig {
    fn default() -> Self {
        Self { slot_offset: DEFAULT_MONITOR_SLOT_OFFSET }
    }
}

/// Returns a spawnable future that monitors pool availability of mined transactions.
pub fn monitor_orderflow_future<N, P, St>(
    pool: P,
    events: St,
    config: OrderflowMonitorConfig,
) -> futures_util::future::BoxFuture<'static, ()>
where
    N: NodePrimitives,
    P: TransactionPool + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        monitor_orderflow(pool, events, config).await;
    }
    .boxed()
}

/// Monitors pool availability of mined transactions.
///
/// For each canonical block, this compares the block's transactions against snapshots of the
/// local pool to determine how many mined transactions were locally available.
async fn monitor_orderflow<N, P, St>(pool: P, mut events: St, config: OrderflowMonitorConfig)
where
    N: NodePrimitives,
    P: TransactionPool + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = OrderflowMonitorMetrics::default();

    // The mid-slot snapshot taken `slot_offset` after the last block.
    let mut mid_slot_snapshot: Option<PoolSnapshot> = None;

    // Timer that fires `slot_offset` after each new canonical block to trigger the mid-slot
    // snapshot.
    let mut snapshot_timer: std::pin::Pin<Box<Sleep>> =
        Box::pin(tokio::time::sleep(config.slot_offset));
    let mut timer_armed = false;

    // The block number we expect the mid-slot snapshot to correspond to (last seen block).
    let mut last_block_number: Option<BlockNumber> = None;

    loop {
        tokio::select! {
            // Mid-slot snapshot timer fires
            _ = &mut snapshot_timer, if timer_armed => {
                timer_armed = false;
                if let Some(block_num) = last_block_number {
                    let pending = pool.pending_transactions();
                    let pending_hashes: HashSet<TxHash> = pending
                        .iter()
                        .map(|tx| *tx.hash())
                        .collect();

                    debug!(
                        target: "txpool::orderflow",
                        block = %block_num,
                        pending_count = %pending_hashes.len(),
                        "mid-slot pool snapshot taken"
                    );

                    mid_slot_snapshot = Some(PoolSnapshot {
                        after_block: block_num,
                        pending_hashes,
                    });
                }
            }

            ev = events.next() => {
                let Some(event) = ev else {
                    break;
                };

                let new = event.committed();
                let (blocks, _state) = new.inner();
                let tip = blocks.tip();
                let tip_number = tip.number();
                let mined_hashes: HashSet<TxHash> = blocks.transaction_hashes().collect();

                if mined_hashes.is_empty() {
                    // empty block, still reset timer
                    last_block_number = Some(tip_number);
                    snapshot_timer
                        .as_mut()
                        .reset(tokio::time::Instant::now() + config.slot_offset);
                    timer_armed = true;
                    continue;
                }

                // --- Mid-slot comparison ---
                // Compare the mid-slot snapshot (taken 4s after the previous block) against
                // this block's transactions.
                if let Some(ref snapshot) = mid_slot_snapshot {
                    let matched = mined_hashes
                        .iter()
                        .filter(|h| snapshot.pending_hashes.contains(*h))
                        .count();

                    let ratio = matched as f64 / mined_hashes.len() as f64;

                    metrics.mid_slot_mined_transactions.set(mined_hashes.len() as f64);
                    metrics.mid_slot_matched_transactions.set(matched as f64);
                    metrics.mid_slot_match_ratio_bps.set((ratio * 10000.0).round());
                    metrics.mid_slot_match_ratio.record(ratio);

                    info!(
                        target: "txpool::orderflow",
                        block = %tip_number,
                        mined = %mined_hashes.len(),
                        matched = %matched,
                        ratio = %format!("{:.2}%", ratio * 100.0),
                        snapshot_after_block = %snapshot.after_block,
                        "mid-slot pool coverage"
                    );
                }

                // --- On-block comparison ---
                // Snapshot of the pool right now (before the pool processes this block).
                let pending = pool.pending_transactions();
                let on_block_hashes: HashSet<TxHash> = pending
                    .iter()
                    .map(|tx| *tx.hash())
                    .collect();

                let matched = mined_hashes
                    .iter()
                    .filter(|h| on_block_hashes.contains(*h))
                    .count();

                let ratio = matched as f64 / mined_hashes.len() as f64;

                metrics.on_block_mined_transactions.set(mined_hashes.len() as f64);
                metrics.on_block_matched_transactions.set(matched as f64);
                metrics.on_block_match_ratio_bps.set((ratio * 10000.0).round());
                metrics.on_block_match_ratio.record(ratio);
                metrics.blocks_monitored.increment(1);

                info!(
                    target: "txpool::orderflow",
                    block = %tip_number,
                    mined = %mined_hashes.len(),
                    matched = %matched,
                    ratio = %format!("{:.2}%", ratio * 100.0),
                    pool_pending = %on_block_hashes.len(),
                    "on-block pool coverage"
                );

                // Clear mid-slot snapshot after use since it was for this block.
                mid_slot_snapshot = None;

                // Arm the timer for the next mid-slot snapshot.
                last_block_number = Some(tip_number);
                snapshot_timer
                    .as_mut()
                    .reset(tokio::time::Instant::now() + config.slot_offset);
                timer_armed = true;
            }
        }
    }
}
