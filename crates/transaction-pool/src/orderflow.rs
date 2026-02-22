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
use futures_util::{Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};
use reth_primitives_traits::NodePrimitives;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, trace};

/// Default offset into the slot at which the mid-slot snapshot is taken.
///
/// 4 seconds matches the Ethereum block building window even though the slot time is 12s.
pub const DEFAULT_MONITOR_SLOT_OFFSET: Duration = Duration::from_secs(4);

/// Maximum age of a mid-slot snapshot before it is considered stale and discarded.
///
/// Slightly above a full slot (12s) + the snapshot offset (4s) to account for minor delays. If a
/// snapshot is older than this when a new block arrives, we were likely doing backfill sync or
/// missed a slot.
const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(13);

/// Maximum allowed difference between a block's timestamp and the wall clock. Blocks older than
/// this are assumed to come from backfill sync rather than live following, so metrics are skipped
/// to avoid polluting results.
const MAX_BLOCK_AGE: Duration = Duration::from_secs(60);

/// Metrics for the transaction pool orderflow monitor.
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
    /// The block number after which this snapshot was taken.
    after_block: BlockNumber,
    /// When this snapshot was taken (monotonic clock).
    taken_at: Instant,
    /// Transaction hashes of all pending (executable) transactions at snapshot time.
    pending_hashes: HashSet<TxHash>,
}

/// Configuration for the orderflow monitor.
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

/// Monitors pool availability of mined transactions.
///
/// For each canonical block, this compares the block's transactions against snapshots of the
/// local pool to determine how many mined transactions were locally available.
pub async fn monitor_orderflow<N, P, St>(pool: P, mut events: St, config: OrderflowMonitorConfig)
where
    N: NodePrimitives,
    P: TransactionPool + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = OrderflowMonitorMetrics::default();

    let mut mid_slot_snapshot: Option<PoolSnapshot> = None;

    // Block number the timer is armed for; `None` means the timer is disarmed.
    let mut mid_slot_target: Option<BlockNumber> = None;

    let sleep = tokio::time::sleep(Duration::MAX);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            _ = &mut sleep, if mid_slot_target.is_some() => {
                if let Some(block_num) = mid_slot_target.take() {
                    let pending_hashes = collect_pending_hashes(&pool);

                    debug!(
                        target: "txpool::orderflow",
                        block = %block_num,
                        pending_count = %pending_hashes.len(),
                        "mid-slot pool snapshot taken"
                    );

                    mid_slot_snapshot = Some(PoolSnapshot {
                        after_block: block_num,
                        taken_at: Instant::now(),
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

                // Skip blocks that are not recent — during backfill sync the pool state
                // doesn't reflect live mempool conditions.
                let now_unix = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let block_age_secs = now_unix.saturating_sub(tip.timestamp());
                if block_age_secs > MAX_BLOCK_AGE.as_secs() {
                    trace!(
                        target: "txpool::orderflow",
                        block = %tip_number,
                        block_age_secs,
                        "skipping stale block during sync"
                    );
                    mid_slot_snapshot = None;
                    mid_slot_target = None;
                    continue;
                }

                // Skip multi-block notifications (catch-up) — the snapshots wouldn't match.
                let first_number = blocks.first().number();
                if first_number != tip_number {
                    debug!(
                        target: "txpool::orderflow",
                        first_block = %first_number,
                        tip_block = %tip_number,
                        "skipping multi-block notification"
                    );
                    mid_slot_snapshot = None;
                    mid_slot_target = Some(tip_number);
                    sleep.as_mut().reset(tokio::time::Instant::now() + config.slot_offset);
                    continue;
                }

                let mined: Vec<TxHash> = blocks.transaction_hashes().collect();

                if mined.is_empty() {
                    mid_slot_snapshot = None;
                    mid_slot_target = Some(tip_number);
                    sleep.as_mut().reset(tokio::time::Instant::now() + config.slot_offset);
                    continue;
                }

                // --- Mid-slot comparison ---
                // Discard snapshot if too old (missed slots or catching up).
                if let Some(ref snapshot) = mid_slot_snapshot {
                    if snapshot.taken_at.elapsed() > MAX_SNAPSHOT_AGE {
                        debug!(
                            target: "txpool::orderflow",
                            block = %tip_number,
                            snapshot_age = ?snapshot.taken_at.elapsed(),
                            snapshot_after_block = %snapshot.after_block,
                            "discarding stale mid-slot snapshot"
                        );
                        mid_slot_snapshot = None;
                    }
                }

                if let Some(ref snapshot) = mid_slot_snapshot {
                    let matched = count_matches(&mined, &snapshot.pending_hashes);
                    let ratio = matched as f64 / mined.len() as f64;
                    let ratio_bps = (ratio * 10000.0).round() as u64;

                    metrics.mid_slot_mined_transactions.set(mined.len() as f64);
                    metrics.mid_slot_matched_transactions.set(matched as f64);
                    metrics.mid_slot_match_ratio_bps.set(ratio_bps as f64);
                    metrics.mid_slot_match_ratio.record(ratio);

                    info!(
                        target: "txpool::orderflow",
                        block = %tip_number,
                        mined = %mined.len(),
                        matched,
                        ratio_bps,
                        snapshot_after_block = %snapshot.after_block,
                        "mid-slot pool coverage"
                    );
                }

                // --- On-block comparison ---
                let on_block_hashes = collect_pending_hashes(&pool);

                let matched = count_matches(&mined, &on_block_hashes);
                let ratio = matched as f64 / mined.len() as f64;
                let ratio_bps = (ratio * 10000.0).round() as u64;

                metrics.on_block_mined_transactions.set(mined.len() as f64);
                metrics.on_block_matched_transactions.set(matched as f64);
                metrics.on_block_match_ratio_bps.set(ratio_bps as f64);
                metrics.on_block_match_ratio.record(ratio);
                metrics.blocks_monitored.increment(1);

                info!(
                    target: "txpool::orderflow",
                    block = %tip_number,
                    mined = %mined.len(),
                    matched,
                    ratio_bps,
                    pool_pending = %on_block_hashes.len(),
                    "on-block pool coverage"
                );

                // Clear mid-slot snapshot after use since it was for this block.
                mid_slot_snapshot = None;

                // Arm the timer for the next mid-slot snapshot.
                mid_slot_target = Some(tip_number);
                sleep.as_mut().reset(tokio::time::Instant::now() + config.slot_offset);
            }
        }
    }
}

/// Collects pending transaction hashes from the pool into a [`HashSet`].
fn collect_pending_hashes<P: TransactionPool>(pool: &P) -> HashSet<TxHash> {
    let pending = pool.pending_transactions();
    pending.iter().map(|tx| *tx.hash()).collect()
}

/// Counts how many hashes from `mined` are present in `pool_hashes`.
fn count_matches(mined: &[TxHash], pool_hashes: &HashSet<TxHash>) -> usize {
    mined.iter().filter(|h| pool_hashes.contains(*h)).count()
}
