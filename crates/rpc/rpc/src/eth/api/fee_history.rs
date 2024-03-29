//! Consist of types adjacent to the fee history cache and its configs

use crate::eth::{cache::EthStateCache, error::EthApiError, gas_oracle::MAX_HEADER_HISTORY};
use futures::{
    future::{Fuse, FusedFuture},
    FutureExt, Stream, StreamExt,
};
use metrics::atomics::AtomicU64;
use reth_primitives::{
    basefee::calculate_next_block_base_fee,
    eip4844::{calc_blob_gasprice, calculate_excess_blob_gas},
    ChainSpec, Receipt, SealedBlock, TransactionSigned, B256, U256,
};
use reth_provider::{BlockReaderIdExt, CanonStateNotification, ChainSpecProvider};
use reth_rpc_types::TxGasAndReward;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Debug,
    sync::{atomic::Ordering::SeqCst, Arc},
};
use tracing::trace;

/// Contains cached fee history entries for blocks.
///
/// Purpose for this is to provide cached data for `eth_feeHistory`.
#[derive(Debug, Clone)]
pub struct FeeHistoryCache {
    inner: Arc<FeeHistoryCacheInner>,
}

impl FeeHistoryCache {
    /// Creates new FeeHistoryCache instance, initialize it with the more recent data, set bounds
    pub fn new(eth_cache: EthStateCache, config: FeeHistoryCacheConfig) -> Self {
        let inner = FeeHistoryCacheInner {
            lower_bound: Default::default(),
            upper_bound: Default::default(),
            config,
            entries: Default::default(),
            eth_cache,
        };
        Self { inner: Arc::new(inner) }
    }

    /// How the cache is configured.
    #[inline]
    pub fn config(&self) -> &FeeHistoryCacheConfig {
        &self.inner.config
    }

    /// Returns the configured resolution for percentile approximation.
    #[inline]
    pub fn resolution(&self) -> u64 {
        self.config().resolution
    }

    /// Returns all blocks that are missing in the cache in the [lower_bound, upper_bound] range.
    ///
    /// This function is used to populate the cache with missing blocks, which can happen if the
    /// node switched to stage sync node.
    async fn missing_consecutive_blocks(&self) -> VecDeque<u64> {
        let entries = self.inner.entries.read().await;
        (self.lower_bound()..self.upper_bound())
            .rev()
            .filter(|&block_number| !entries.contains_key(&block_number))
            .collect()
    }

    /// Insert block data into the cache.
    async fn insert_blocks<I>(&self, blocks: I)
    where
        I: IntoIterator<Item = (SealedBlock, Arc<Vec<Receipt>>)>,
    {
        let mut entries = self.inner.entries.write().await;

        let percentiles = self.predefined_percentiles();
        // Insert all new blocks and calculate approximated rewards
        for (block, receipts) in blocks {
            let mut fee_history_entry = FeeHistoryEntry::new(&block);
            fee_history_entry.rewards = calculate_reward_percentiles_for_block(
                &percentiles,
                fee_history_entry.gas_used,
                fee_history_entry.base_fee_per_gas,
                &block.body,
                &receipts,
            )
            .unwrap_or_default();
            entries.insert(block.number, fee_history_entry);
        }

        // enforce bounds by popping the oldest entries
        while entries.len() > self.inner.config.max_blocks as usize {
            entries.pop_first();
        }

        if entries.len() == 0 {
            self.inner.upper_bound.store(0, SeqCst);
            self.inner.lower_bound.store(0, SeqCst);
            return
        }

        let upper_bound = *entries.last_entry().expect("Contains at least one entry").key();

        // also enforce proper lower bound in case we have gaps
        let target_lower = upper_bound.saturating_sub(self.inner.config.max_blocks);
        while entries.len() > 1 && *entries.first_key_value().unwrap().0 < target_lower {
            entries.pop_first();
        }

        let lower_bound = *entries.first_entry().expect("Contains at least one entry").key();
        self.inner.upper_bound.store(upper_bound, SeqCst);
        self.inner.lower_bound.store(lower_bound, SeqCst);
    }

    /// Get UpperBound value for FeeHistoryCache
    pub fn upper_bound(&self) -> u64 {
        self.inner.upper_bound.load(SeqCst)
    }

    /// Get LowerBound value for FeeHistoryCache
    pub fn lower_bound(&self) -> u64 {
        self.inner.lower_bound.load(SeqCst)
    }

    /// Collect fee history for given range.
    ///
    /// This function retrieves fee history entries from the cache for the specified range.
    /// If the requested range (start_block to end_block) is within the cache bounds,
    /// it returns the corresponding entries.
    /// Otherwise it returns None.
    pub async fn get_history(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Option<Vec<FeeHistoryEntry>> {
        let lower_bound = self.lower_bound();
        let upper_bound = self.upper_bound();
        if start_block >= lower_bound && end_block <= upper_bound {
            let entries = self.inner.entries.read().await;
            let result = entries
                .range(start_block..=end_block)
                .map(|(_, fee_entry)| fee_entry.clone())
                .collect::<Vec<_>>();

            if result.is_empty() {
                return None
            }

            Some(result)
        } else {
            None
        }
    }

    /// Generates predefined set of percentiles
    ///
    /// This returns 100 * resolution points
    pub fn predefined_percentiles(&self) -> Vec<f64> {
        let res = self.resolution() as f64;
        (0..=100 * self.resolution()).map(|p| p as f64 / res).collect()
    }
}

/// Settings for the [FeeHistoryCache].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHistoryCacheConfig {
    /// Max number of blocks in cache.
    ///
    /// Default is [MAX_HEADER_HISTORY] plus some change to also serve slightly older blocks from
    /// cache, since fee_history supports the entire range
    pub max_blocks: u64,
    /// Percentile approximation resolution
    ///
    /// Default is 4 which means 0.25
    pub resolution: u64,
}

impl Default for FeeHistoryCacheConfig {
    fn default() -> Self {
        FeeHistoryCacheConfig { max_blocks: MAX_HEADER_HISTORY + 100, resolution: 4 }
    }
}

/// Container type for shared state in [FeeHistoryCache]
#[derive(Debug)]
struct FeeHistoryCacheInner {
    /// Stores the lower bound of the cache
    lower_bound: AtomicU64,
    /// Stores the upper bound of the cache
    upper_bound: AtomicU64,
    /// Config for FeeHistoryCache, consists of resolution for percentile approximation
    /// and max number of blocks
    config: FeeHistoryCacheConfig,
    /// Stores the entries of the cache
    entries: tokio::sync::RwLock<BTreeMap<u64, FeeHistoryEntry>>,
    eth_cache: EthStateCache,
}

/// Awaits for new chain events and directly inserts them into the cache so they're available
/// immediately before they need to be fetched from disk.
pub async fn fee_history_cache_new_blocks_task<St, Provider>(
    fee_history_cache: FeeHistoryCache,
    mut events: St,
    provider: Provider,
) where
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
    Provider: BlockReaderIdExt + ChainSpecProvider + 'static,
{
    // We're listening for new blocks emitted when the node is in live sync.
    // If the node transitions to stage sync, we need to fetch the missing blocks
    let mut missing_blocks = VecDeque::new();
    let mut fetch_missing_block = Fuse::terminated();

    loop {
        if fetch_missing_block.is_terminated() {
            if let Some(block_number) = missing_blocks.pop_front() {
                trace!(target: "rpc::fee", ?block_number, "Fetching missing block for fee history cache");
                if let Ok(Some(hash)) = provider.block_hash(block_number) {
                    // fetch missing block
                    fetch_missing_block = fee_history_cache
                        .inner
                        .eth_cache
                        .get_block_and_receipts(hash)
                        .boxed()
                        .fuse();
                }
            }
        }

        tokio::select! {
            res = &mut fetch_missing_block =>  {
                if let Ok(res) = res {
                    fee_history_cache.insert_blocks(res.into_iter()).await;
                }
            }
            event = events.next() =>  {
                let Some(event) = event else {
                     // the stream ended, we are done
                    break;
                };
                if let Some(committed) = event.committed() {
                    let (blocks, receipts): (Vec<_>, Vec<_>) = committed
                        .blocks_and_receipts()
                        .map(|(block, receipts)| {
                            (block.block.clone(), Arc::new(receipts.iter().flatten().cloned().collect::<Vec<_>>()))
                        })
                        .unzip();
                    fee_history_cache.insert_blocks(blocks.into_iter().zip(receipts)).await;

                    // keep track of missing blocks
                    missing_blocks = fee_history_cache.missing_consecutive_blocks().await;
                }
            }
        }
    }
}

/// Calculates reward percentiles for transactions in a block header.
/// Given a list of percentiles and a sealed block header, this function computes
/// the corresponding rewards for the transactions at each percentile.
///
/// The results are returned as a vector of U256 values.
pub(crate) fn calculate_reward_percentiles_for_block(
    percentiles: &[f64],
    gas_used: u64,
    base_fee_per_gas: u64,
    transactions: &[TransactionSigned],
    receipts: &[Receipt],
) -> Result<Vec<U256>, EthApiError> {
    let mut transactions = transactions
        .iter()
        .zip(receipts)
        .scan(0, |previous_gas, (tx, receipt)| {
            // Convert the cumulative gas used in the receipts
            // to the gas usage by the transaction
            //
            // While we will sum up the gas again later, it is worth
            // noting that the order of the transactions will be different,
            // so the sum will also be different for each receipt.
            let gas_used = receipt.cumulative_gas_used - *previous_gas;
            *previous_gas = receipt.cumulative_gas_used;

            Some(TxGasAndReward {
                gas_used,
                reward: tx.effective_tip_per_gas(Some(base_fee_per_gas)).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    // Sort the transactions by their rewards in ascending order
    transactions.sort_by_key(|tx| tx.reward);

    // Find the transaction that corresponds to the given percentile
    //
    // We use a `tx_index` here that is shared across all percentiles, since we know
    // the percentiles are monotonically increasing.
    let mut tx_index = 0;
    let mut cumulative_gas_used = transactions.first().map(|tx| tx.gas_used).unwrap_or_default();
    let mut rewards_in_block = Vec::new();
    for percentile in percentiles {
        // Empty blocks should return in a zero row
        if transactions.is_empty() {
            rewards_in_block.push(U256::ZERO);
            continue
        }

        let threshold = (gas_used as f64 * percentile / 100.) as u64;
        while cumulative_gas_used < threshold && tx_index < transactions.len() - 1 {
            tx_index += 1;
            cumulative_gas_used += transactions[tx_index].gas_used;
        }
        rewards_in_block.push(U256::from(transactions[tx_index].reward));
    }

    Ok(rewards_in_block)
}

/// A cached entry for a block's fee history.
#[derive(Debug, Clone)]
pub struct FeeHistoryEntry {
    /// The base fee per gas for this block.
    pub base_fee_per_gas: u64,
    /// Gas used ratio this block.
    pub gas_used_ratio: f64,
    /// The base per blob gas for EIP-4844.
    /// For pre EIP-4844 equals to zero.
    pub base_fee_per_blob_gas: Option<u128>,
    /// Blob gas used ratio for this block.
    ///
    /// Calculated as the ratio of blob gas used and the available blob data gas per block.
    /// Will be zero if no blob gas was used or pre EIP-4844.
    pub blob_gas_used_ratio: f64,
    /// The excess blob gas of the block.
    pub excess_blob_gas: Option<u64>,
    /// The total amount of blob gas consumed by the transactions within the block,
    /// added in EIP-4844
    pub blob_gas_used: Option<u64>,
    /// Gas used by this block.
    pub gas_used: u64,
    /// Gas limit by this block.
    pub gas_limit: u64,
    /// Hash of the block.
    pub header_hash: B256,
    /// Approximated rewards for the configured percentiles.
    pub rewards: Vec<U256>,
    /// The timestamp of the block.
    pub timestamp: u64,
}

impl FeeHistoryEntry {
    /// Creates a new entry from a sealed block.
    ///
    /// Note: This does not calculate the rewards for the block.
    pub fn new(block: &SealedBlock) -> Self {
        FeeHistoryEntry {
            base_fee_per_gas: block.base_fee_per_gas.unwrap_or_default(),
            gas_used_ratio: block.gas_used as f64 / block.gas_limit as f64,
            base_fee_per_blob_gas: block.blob_fee(),
            blob_gas_used_ratio: block.blob_gas_used() as f64 /
                reth_primitives::constants::eip4844::MAX_DATA_GAS_PER_BLOCK as f64,
            excess_blob_gas: block.excess_blob_gas,
            blob_gas_used: block.blob_gas_used,
            gas_used: block.gas_used,
            header_hash: block.hash(),
            gas_limit: block.gas_limit,
            rewards: Vec::new(),
            timestamp: block.timestamp,
        }
    }

    /// Returns the base fee for the next block according to the EIP-1559 spec.
    pub fn next_block_base_fee(&self, chain_spec: &ChainSpec) -> u64 {
        calculate_next_block_base_fee(
            self.gas_used,
            self.gas_limit,
            self.base_fee_per_gas,
            chain_spec.base_fee_params(self.timestamp),
        )
    }

    /// Returns the blob fee for the next block according to the EIP-4844 spec.
    ///
    /// Returns `None` if `excess_blob_gas` is None.
    ///
    /// See also [Self::next_block_excess_blob_gas]
    pub fn next_block_blob_fee(&self) -> Option<u128> {
        self.next_block_excess_blob_gas().map(calc_blob_gasprice)
    }

    /// Calculate excess blob gas for the next block according to the EIP-4844 spec.
    ///
    /// Returns a `None` if no excess blob gas is set, no EIP-4844 support
    pub fn next_block_excess_blob_gas(&self) -> Option<u64> {
        Some(calculate_excess_blob_gas(self.excess_blob_gas?, self.blob_gas_used?))
    }
}
