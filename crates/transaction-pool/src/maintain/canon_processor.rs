//! Support for processing canonical state updates for the transaction pool

use crate::{
    blobstore::{BlobStoreCanonTracker, BlobStoreUpdates},
    maintain::drift_monitor::{load_accounts, DriftMonitor, LoadedAccounts, PoolDriftState},
    metrics::MaintainPoolMetrics,
    traits::{CanonicalStateUpdate, EthPoolTransaction, TransactionPool, TransactionPoolExt},
    BlockInfo, PoolTransaction, PoolUpdateKind,
};
use alloy_consensus::BlockHeader;
use alloy_eips::Typed2718;
use alloy_primitives::BlockNumber;
use reth_chain_state::CanonStateNotification;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_execution_types::ChangedAccount;
use reth_primitives_traits::{NodePrimitives, SignedTransaction};
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, trace};

/// Tracks the last finalized block for update processing
#[derive(Debug, Clone)]
pub struct FinalizedBlockTracker {
    last_finalized_block: Option<BlockNumber>,
}

impl FinalizedBlockTracker {
    /// Create a new finalized block tracker
    pub const fn new(last_finalized_block: Option<BlockNumber>) -> Self {
        Self { last_finalized_block }
    }

    /// Updates the tracked finalized block and returns the new finalized block if it changed
    pub fn update(&mut self, finalized_block: Option<BlockNumber>) -> Option<BlockNumber> {
        let finalized = finalized_block?;
        self.last_finalized_block
            .replace(finalized)
            .is_none_or(|last| last < finalized)
            .then_some(finalized)
    }
}

/// A unique [`ChangedAccount`] identified by its address that can be used for deduplication
#[derive(Eq, Debug)]
pub(crate) struct ChangedAccountEntry(pub ChangedAccount);

impl PartialEq for ChangedAccountEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.address == other.0.address
    }
}

impl std::hash::Hash for ChangedAccountEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.address.hash(state);
    }
}

impl std::borrow::Borrow<alloy_primitives::Address> for ChangedAccountEntry {
    fn borrow(&self) -> &alloy_primitives::Address {
        &self.0.address
    }
}

/// Configuration for the canonical event processor
#[derive(Debug, Clone, Copy)]
pub struct CanonEventProcessorConfig {
    /// Maximum depth for updates before marking as drifted
    pub max_update_depth: u64,
}

impl Default for CanonEventProcessorConfig {
    fn default() -> Self {
        Self { max_update_depth: 64 }
    }
}

/// Processor for canonical state events in the transaction pool
#[derive(Debug, Clone)]
pub struct CanonEventProcessor<N>
where
    N: NodePrimitives,
{
    /// The finalized block tracker
    finalized_tracker: FinalizedBlockTracker,
    /// The blob store tracker
    blob_store_tracker: BlobStoreCanonTracker,
    /// Configuration for the processor
    config: CanonEventProcessorConfig,
    /// Metrics for the processor
    metrics: MaintainPoolMetrics,
    /// Type marker
    _marker: std::marker::PhantomData<N>,
}

impl<N> CanonEventProcessor<N>
where
    N: NodePrimitives,
{
    /// Create a new canon event processor
    pub fn new(
        finalized_block: Option<BlockNumber>,
        config: CanonEventProcessorConfig,
        metrics: MaintainPoolMetrics,
    ) -> Self {
        Self {
            finalized_tracker: FinalizedBlockTracker::new(finalized_block),
            blob_store_tracker: BlobStoreCanonTracker::default(),
            config,
            metrics,
            _marker: std::marker::PhantomData,
        }
    }

    /// Returns a Future for processing an event
    ///
    /// This is used by the `PoolMaintainer` to manually poll the event processing
    pub fn process_event<'a, Client, P>(
        &'a mut self,
        event: CanonStateNotification<N>,
        client: &'a Client,
        pool: &'a P,
        drift_monitor: &'a mut DriftMonitor,
    ) -> impl std::future::Future<Output = ()> + 'a
    where
        Client: reth_storage_api::StateProviderFactory + ChainSpecProvider + Clone + 'static,
        P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    {
        let event_clone = event.clone();
        async move {
            if !self.process_reorg(event_clone, client, pool, drift_monitor).await {
                // if not a reorg, try as a commit
                self.process_commit(event, client, pool, drift_monitor);
            }
        }
    }

    /// Returns a Future for processing an event with a generic drift monitor
    ///
    /// This is used by custom implementations with the `DriftMonitoring` trait
    pub fn process_event_with_monitor<'a, Client, P, M>(
        &'a mut self,
        event: CanonStateNotification<N>,
        client: &'a Client,
        pool: &'a P,
        drift_monitor: &'a mut M,
    ) -> impl std::future::Future<Output = ()> + 'a
    where
        Client: reth_storage_api::StateProviderFactory + ChainSpecProvider + Clone + 'static,
        P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
        M: super::interfaces::DriftMonitoring<Client> + 'a,
    {
        let event_clone = event.clone();
        async move {
            if !self.process_reorg(event_clone, client, pool, drift_monitor).await {
                // if not a reorg, try as a commit
                self.process_commit(event, client, pool, drift_monitor);
            }
        }
    }

    /// Update finalized blocks and return finalized blob hashes if any
    pub fn update_finalized<Client>(&mut self, client: &Client) -> Option<BlobStoreUpdates>
    where
        Client: reth_storage_api::BlockReaderIdExt,
    {
        let finalized =
            self.finalized_tracker.update(client.finalized_block_number().ok().flatten())?;

        let updates = self.blob_store_tracker.on_finalized_block(finalized);

        // Increment metrics for finalized blobs if they exist
        if let BlobStoreUpdates::Finalized(ref blobs) = updates {
            self.metrics.inc_deleted_tracked_blobs(blobs.len());
        }

        Some(updates)
    }

    /// Process a reorg event with a generic drift monitor implementing `DriftMonitoring`
    pub(crate) async fn process_reorg<Client, P, M>(
        &mut self,
        event: CanonStateNotification<N>,
        client: &Client,
        pool: &P,
        drift_monitor: &mut M,
    ) -> bool
    where
        Client: reth_storage_api::StateProviderFactory + ChainSpecProvider + Clone + 'static,
        P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
        M: super::interfaces::DriftMonitoring<Client>,
    {
        match event {
            CanonStateNotification::Reorg { old, new } => {
                let (old_blocks, old_state) = old.inner();
                let (new_blocks, new_state) = new.inner();
                let new_tip = new_blocks.tip();
                let new_first = new_blocks.first();
                let old_first = old_blocks.first();
                let pool_info = pool.block_info();

                // check if the reorg is not canonical with the pool's block
                if !(old_first.parent_hash() == pool_info.last_seen_block_hash ||
                    new_first.parent_hash() == pool_info.last_seen_block_hash)
                {
                    // the new block points to a higher block than the oldest block in the old chain
                    drift_monitor.set_state(PoolDriftState::Drifted);
                }

                let chain_spec = client.chain_spec();

                // fees for the next block: `new_tip+1`
                let pending_block_base_fee = new_tip
                    .header()
                    .next_block_base_fee(
                        chain_spec.base_fee_params_at_timestamp(new_tip.timestamp()),
                    )
                    .unwrap_or_default();
                let pending_block_blob_fee = new_tip.header().maybe_next_block_blob_fee(
                    chain_spec.blob_params_at_timestamp(new_tip.timestamp()),
                );

                // we know all changed account in the new chain
                let new_changed_accounts: HashSet<_> =
                    new_state.changed_accounts().map(ChangedAccountEntry).collect();

                // find all accounts that were changed in the old chain but _not_ in the new chain
                let missing_changed_acc = old_state
                    .accounts_iter()
                    .map(|(a, _)| a)
                    .filter(|addr| !new_changed_accounts.contains(addr));

                // for these we need to fetch the nonce+balance from the db at the new tip
                let mut changed_accounts =
                    match load_accounts(client.clone(), new_tip.hash(), missing_changed_acc) {
                        Ok(LoadedAccounts { accounts, failed_to_load }) => {
                            // extend accounts we failed to load from database
                            drift_monitor.add_dirty_addresses(failed_to_load);

                            accounts
                        }
                        Err(err) => {
                            let (addresses, err) = *err;
                            debug!(
                                target: "txpool",
                                %err,
                                "failed to load missing changed accounts at new tip: {:?}",
                                new_tip.hash()
                            );
                            drift_monitor.add_dirty_addresses(addresses);
                            vec![]
                        }
                    };

                // also include all accounts from new chain
                // we can use extend here because they are unique
                changed_accounts.extend(new_changed_accounts.into_iter().map(|entry| entry.0));

                // all transactions mined in the new chain
                let new_mined_transactions: HashSet<_> = new_blocks.transaction_hashes().collect();

                // update the pool then re-inject the pruned transactions
                // find all transactions that were mined in the old chain but not in the new chain
                let pruned_old_transactions = old_blocks
                    .transactions_ecrecovered()
                    .filter(|tx| !new_mined_transactions.contains(tx.tx_hash()))
                    .filter_map(|tx| {
                        if tx.is_eip4844() {
                            // reorged blobs no longer include the blob, which is necessary for
                            // validating the transaction. Even though the transaction could have
                            // been validated previously, we still need the blob in order to
                            // accurately set the transaction's
                            // encoded-length which is propagated over the network.
                            pool.get_blob(*tx.tx_hash())
                                .ok()
                                .flatten()
                                .map(Arc::unwrap_or_clone)
                                .and_then(|sidecar| {
                                    <P as TransactionPool>::Transaction::try_from_eip4844(
                                        tx, sidecar,
                                    )
                                })
                        } else {
                            <P as TransactionPool>::Transaction::try_from_consensus(tx).ok()
                        }
                    })
                    .collect::<Vec<_>>();

                // update the pool first
                let update = CanonicalStateUpdate {
                    new_tip: new_tip.sealed_block(),
                    pending_block_base_fee,
                    pending_block_blob_fee,
                    changed_accounts,
                    // all transactions mined in the new chain need to be removed from the pool
                    mined_transactions: new_blocks.transaction_hashes().collect(),
                    update_kind: PoolUpdateKind::Reorg,
                };
                pool.on_canonical_state_change(update);

                // all transactions that were mined in the old chain but not in the new chain need
                // to be re-injected
                //
                // Note: we no longer know if the tx was local or external
                // Because the transactions are not finalized, the corresponding blobs are still in
                // blob store (if we previously received them from the network)
                self.metrics.inc_reinserted_transactions(pruned_old_transactions.len());
                let _ = pool.add_external_transactions(pruned_old_transactions).await;

                // keep track of new mined blob transactions
                self.blob_store_tracker.add_new_chain_blocks(&new_blocks);

                true
            }
            _ => false,
        }
    }

    /// Process a commit event with a generic drift monitor implementing `DriftMonitoring`
    pub(crate) fn process_commit<Client, P, M>(
        &mut self,
        event: CanonStateNotification<N>,
        client: &Client,
        pool: &P,
        drift_monitor: &mut M,
    ) -> bool
    where
        Client: ChainSpecProvider,
        P: TransactionPoolExt,
        M: super::interfaces::DriftMonitoring<Client>,
    {
        match event {
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();
                let chain_spec = client.chain_spec();
                let pool_info = pool.block_info();

                // fees for the next block: `tip+1`
                let pending_block_base_fee = tip
                    .header()
                    .next_block_base_fee(chain_spec.base_fee_params_at_timestamp(tip.timestamp()))
                    .unwrap_or_default();
                let pending_block_blob_fee = tip.header().maybe_next_block_blob_fee(
                    chain_spec.blob_params_at_timestamp(tip.timestamp()),
                );

                let first_block = blocks.first();
                trace!(
                    target: "txpool",
                    first = first_block.number(),
                    tip = tip.number(),
                    pool_block = pool_info.last_seen_block_number,
                    "update pool on new commit"
                );

                // check if the depth is too large and should be skipped, this could happen after
                // initial sync or long re-sync
                let depth = tip.number().abs_diff(pool_info.last_seen_block_number);
                if depth > self.config.max_update_depth {
                    drift_monitor.set_state(PoolDriftState::Drifted);
                    debug!(target: "txpool", ?depth, "skipping deep canonical update");
                    let info = BlockInfo {
                        block_gas_limit: tip.header().gas_limit(),
                        last_seen_block_hash: tip.hash(),
                        last_seen_block_number: tip.number(),
                        pending_basefee: pending_block_base_fee,
                        pending_blob_fee: pending_block_blob_fee,
                    };
                    pool.set_block_info(info);

                    // keep track of mined blob transactions
                    self.blob_store_tracker.add_new_chain_blocks(&blocks);

                    return true
                }

                let mut changed_accounts = Vec::with_capacity(state.state().len());
                for acc in state.changed_accounts() {
                    // we can always clear the dirty flag for this account
                    drift_monitor.remove_dirty_address(&acc.address);
                    changed_accounts.push(acc);
                }

                let mined_transactions = blocks.transaction_hashes().collect();

                // check if the range of the commit is canonical with the pool's block
                if first_block.parent_hash() != pool_info.last_seen_block_hash {
                    // we received a new canonical chain commit but the commit is not canonical with
                    // the pool's block, this could happen after initial sync or
                    // long re-sync
                    drift_monitor.set_state(PoolDriftState::Drifted);
                }

                // Canonical update
                let update = CanonicalStateUpdate {
                    new_tip: tip.sealed_block(),
                    pending_block_base_fee,
                    pending_block_blob_fee,
                    changed_accounts,
                    mined_transactions,
                    update_kind: PoolUpdateKind::Commit,
                };
                pool.on_canonical_state_change(update);

                // keep track of mined blob transactions
                self.blob_store_tracker.add_new_chain_blocks(&blocks);

                true
            }
            _ => false,
        }
    }

    /// Get finalized blob hashes for cleanup
    pub fn get_finalized_blobs(&mut self, finalized: BlockNumber) -> BlobStoreUpdates {
        self.blob_store_tracker.on_finalized_block(finalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_with_higher_finalized_block() {
        let mut tracker = FinalizedBlockTracker::new(Some(10));
        assert_eq!(tracker.update(Some(15)), Some(15));
        assert_eq!(tracker.last_finalized_block, Some(15));
    }

    #[test]
    fn test_update_with_lower_finalized_block() {
        let mut tracker = FinalizedBlockTracker::new(Some(20));
        assert_eq!(tracker.update(Some(15)), None);
        assert_eq!(tracker.last_finalized_block, Some(15));
    }

    #[test]
    fn test_update_with_equal_finalized_block() {
        let mut tracker = FinalizedBlockTracker::new(Some(20));
        assert_eq!(tracker.update(Some(20)), None);
        assert_eq!(tracker.last_finalized_block, Some(20));
    }

    #[test]
    fn test_update_with_no_last_finalized_block() {
        let mut tracker = FinalizedBlockTracker::new(None);
        assert_eq!(tracker.update(Some(10)), Some(10));
        assert_eq!(tracker.last_finalized_block, Some(10));
    }

    #[test]
    fn test_update_with_no_new_finalized_block() {
        let mut tracker = FinalizedBlockTracker::new(Some(10));
        assert_eq!(tracker.update(None), None);
        assert_eq!(tracker.last_finalized_block, Some(10));
    }

    #[test]
    fn test_update_with_no_finalized_blocks() {
        let mut tracker = FinalizedBlockTracker::new(None);
        assert_eq!(tracker.update(None), None);
        assert_eq!(tracker.last_finalized_block, None);
    }
}
