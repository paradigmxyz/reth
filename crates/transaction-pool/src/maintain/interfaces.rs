//! Interfaces for the transaction pool maintenance components
//!
//! This module defines traits for the key components in the transaction pool
//! maintenance system, allowing them to be replaced with custom implementations.

use crate::{
    blobstore::BlobStoreUpdates,
    maintain::{
        canon_processor::CanonEventProcessor,
        drift_monitor::{DriftMonitor, DriftMonitorResult, PoolDriftState},
    },
    traits::TransactionPoolExt,
    PoolTransaction,
};
use alloy_primitives::{Address, BlockHash, BlockNumber};
use futures_util::Future;
use reth_chain_state::CanonStateNotification;
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::NodePrimitives;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use std::{collections::HashSet, pin::Pin};

/// Trait defining the interface for a drift monitor
pub trait DriftMonitoring<Client>: Future<Output = DriftMonitorResult> {
    /// Returns true if the monitor is currently reloading accounts
    fn is_reloading(&self) -> bool;

    /// Returns the current drift state
    fn state(&self) -> PoolDriftState;

    /// Set the drift state
    fn set_state(&mut self, state: PoolDriftState);

    /// Mark the first event as received
    fn on_first_event(&mut self);

    /// Set dirty addresses to be reloaded
    fn set_dirty_addresses(&mut self, addresses: HashSet<Address>);

    /// Add addresses to the dirty set
    fn add_dirty_addresses<I>(&mut self, addresses: I)
    where
        I: IntoIterator<Item = Address>;

    /// Remove an address from the dirty set
    fn remove_dirty_address(&mut self, address: &Address) -> bool;

    /// Check if there are dirty addresses
    fn has_dirty_addresses(&self) -> bool;

    /// Start reloading addresses from state
    fn start_reload_accounts<Spawner>(
        &mut self,
        client: Client,
        at: BlockHash,
        task_spawner: &Spawner,
    ) where
        Client: StateProviderFactory + Clone + 'static,
        Spawner: reth_tasks::TaskSpawner + 'static;

    /// Update metrics
    fn update_metrics(&self);
}

impl<Client> DriftMonitoring<Client> for DriftMonitor
where
    Client: 'static,
{
    fn is_reloading(&self) -> bool {
        Self::is_reloading(self)
    }

    fn state(&self) -> PoolDriftState {
        Self::state(self)
    }

    fn set_state(&mut self, state: PoolDriftState) {
        Self::set_state(self, state)
    }

    fn on_first_event(&mut self) {
        Self::on_first_event(self)
    }

    fn set_dirty_addresses(&mut self, addresses: HashSet<Address>) {
        Self::set_dirty_addresses(self, addresses)
    }

    fn add_dirty_addresses<I>(&mut self, addresses: I)
    where
        I: IntoIterator<Item = Address>,
    {
        Self::add_dirty_addresses(self, addresses)
    }

    fn remove_dirty_address(&mut self, address: &Address) -> bool {
        Self::remove_dirty_address(self, address)
    }

    fn has_dirty_addresses(&self) -> bool {
        Self::has_dirty_addresses(self)
    }

    fn start_reload_accounts<Spawner>(
        &mut self,
        client: Client,
        at: BlockHash,
        task_spawner: &Spawner,
    ) where
        Client: StateProviderFactory + Clone + 'static,
        Spawner: reth_tasks::TaskSpawner + 'static,
    {
        Self::start_reload_accounts(self, client, at, task_spawner)
    }

    fn update_metrics(&self) {
        Self::update_metrics(self)
    }
}

/// Trait defining the interface for canonical chain event processing
pub trait CanonProcessing<N, Client, P>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
{
    /// Process a canonical chain event
    fn process_event<'a, M>(
        &'a mut self,
        event: CanonStateNotification<N>,
        client: &'a Client,
        pool: &'a P,
        drift_monitor: &'a mut M,
    ) -> Pin<Box<dyn Future<Output = ()> + 'a>>
    where
        M: DriftMonitoring<Client> + 'a;

    /// Update finalized blocks and return finalized blob hashes if any
    fn update_finalized(&mut self, client: &Client) -> Option<BlobStoreUpdates>;

    /// Get finalized blob hashes for cleanup
    fn get_finalized_blobs(&mut self, finalized: BlockNumber) -> BlobStoreUpdates;
}

impl<N, Client, P> CanonProcessing<N, Client, P> for CanonEventProcessor<N>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
{
    fn process_event<'a, M>(
        &'a mut self,
        event: CanonStateNotification<N>,
        client: &'a Client,
        pool: &'a P,
        drift_monitor: &'a mut M,
    ) -> Pin<Box<dyn Future<Output = ()> + 'a>>
    where
        M: DriftMonitoring<Client> + 'a,
    {
        Box::pin(async move {
            if !self.process_reorg(event.clone(), client, pool, drift_monitor).await {
                // if not a reorg, try as a commit
                self.process_commit(event, client, pool, drift_monitor);
            }
        })
    }

    fn update_finalized(&mut self, client: &Client) -> Option<BlobStoreUpdates> {
        self.update_finalized(client)
    }

    fn get_finalized_blobs(&mut self, finalized: BlockNumber) -> BlobStoreUpdates {
        self.get_finalized_blobs(finalized)
    }
}

/// Pool maintainer component factory
///
/// This trait allows creating custom pool maintainer components
pub trait PoolMaintainerComponentFactory<N, Client, P, M, C>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    M: DriftMonitoring<Client>,
    C: CanonProcessing<N, Client, P>,
{
    /// Create a drift monitor
    fn create_drift_monitor(
        &self,
        max_reload_accounts: usize,
        metrics: crate::metrics::MaintainPoolMetrics,
    ) -> M;

    /// Create a canon processor
    fn create_canon_processor(
        &self,
        finalized_block: Option<BlockNumber>,
        config: crate::maintain::canon_processor::CanonEventProcessorConfig,
        metrics: crate::metrics::MaintainPoolMetrics,
    ) -> C;
}

/// Default implementation of the component factory
#[derive(Debug)]
pub struct DefaultComponentFactory;

impl<N, Client, P>
    PoolMaintainerComponentFactory<N, Client, P, DriftMonitor, CanonEventProcessor<N>>
    for DefaultComponentFactory
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
{
    fn create_drift_monitor(
        &self,
        max_reload_accounts: usize,
        metrics: crate::metrics::MaintainPoolMetrics,
    ) -> DriftMonitor {
        DriftMonitor::new(max_reload_accounts, metrics)
    }

    fn create_canon_processor(
        &self,
        finalized_block: Option<BlockNumber>,
        config: crate::maintain::canon_processor::CanonEventProcessorConfig,
        metrics: crate::metrics::MaintainPoolMetrics,
    ) -> crate::maintain::canon_processor::CanonEventProcessor<N> {
        crate::maintain::canon_processor::CanonEventProcessor::new(finalized_block, config, metrics)
    }
}
