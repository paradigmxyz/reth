//! Support for monitoring and handling drift in the transaction pool

use crate::metrics::MaintainPoolMetrics;
use alloy_primitives::{Address, BlockHash};
use futures_util::{
    future::{Fuse, FusedFuture},
    FutureExt,
};
use reth_execution_types::ChangedAccount;
use reth_storage_api::{errors::provider::ProviderError, StateProviderFactory};
use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tracing::debug;

/// Result of loading accounts from state
#[derive(Default, Debug)]
pub struct LoadedAccounts {
    /// All accounts that were loaded
    pub accounts: Vec<ChangedAccount>,
    /// All accounts that failed to load
    pub failed_to_load: Vec<Address>,
}

/// Loads all accounts at the given state
///
/// Returns an error with all given addresses if the state is not available.
///
/// Note: this expects _unique_ addresses
pub(crate) fn load_accounts<Client, I>(
    client: Client,
    at: BlockHash,
    addresses: I,
) -> Result<LoadedAccounts, Box<(HashSet<Address>, ProviderError)>>
where
    I: IntoIterator<Item = Address>,
    Client: StateProviderFactory,
{
    let addresses = addresses.into_iter();
    let mut res = LoadedAccounts::default();
    let state = match client.history_by_block_hash(at) {
        Ok(state) => state,
        Err(err) => return Err(Box::new((addresses.collect(), err))),
    };
    for addr in addresses {
        if let Ok(maybe_acc) = state.basic_account(&addr) {
            let acc = maybe_acc
                .map(|acc| ChangedAccount { address: addr, nonce: acc.nonce, balance: acc.balance })
                .unwrap_or_else(|| ChangedAccount::empty(addr));
            res.accounts.push(acc)
        } else {
            // failed to load account.
            res.failed_to_load.push(addr);
        }
    }
    Ok(res)
}

/// Keeps track of the pool's state, whether the accounts in the pool are in sync with the actual
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolDriftState {
    /// Pool is assumed to be in sync with the current state
    InSync,
    /// Pool could be out of sync with the state
    Drifted,
}

impl PoolDriftState {
    /// Returns `true` if the pool is assumed to be out of sync with the current state.
    #[inline]
    pub const fn is_drifted(&self) -> bool {
        matches!(self, Self::Drifted)
    }
}

/// The result of a drift monitoring operation
#[derive(Debug)]
pub enum DriftMonitorResult {
    /// Accounts were loaded successfully
    AccountsLoaded(LoadedAccounts),
    /// The account reload future failed
    Failed,
}

type ReloadAccountsFut =
    Fuse<oneshot::Receiver<Result<LoadedAccounts, Box<(HashSet<Address>, ProviderError)>>>>;

/// Monitors and handles transaction pool drift from the canonical state
#[derive(Debug)]
pub struct DriftMonitor {
    /// Current state of drift
    state: PoolDriftState,
    /// Set of addresses that need to be reloaded
    dirty_addresses: HashSet<Address>,
    /// Maximum number of accounts to reload at once
    max_reload_accounts: usize,
    /// The future that reloads accounts from state
    reload_accounts_fut: ReloadAccountsFut,
    /// Whether this is the first event received
    first_event: bool,
    /// Metrics for monitoring
    metrics: MaintainPoolMetrics,
}

impl DriftMonitor {
    /// Create a new drift monitor
    pub fn new(max_reload_accounts: usize, metrics: MaintainPoolMetrics) -> Self {
        Self {
            state: PoolDriftState::InSync,
            dirty_addresses: HashSet::default(),
            max_reload_accounts,
            reload_accounts_fut: Fuse::terminated(),
            first_event: true,
            metrics,
        }
    }

    /// Returns `true` if the monitor is currently reloading accounts
    pub fn is_reloading(&self) -> bool {
        !self.reload_accounts_fut.is_terminated()
    }

    /// Returns the current drift state
    pub const fn state(&self) -> PoolDriftState {
        self.state
    }

    /// Set the drift state
    pub fn set_state(&mut self, state: PoolDriftState) {
        self.state = state;
        if state.is_drifted() {
            self.metrics.inc_drift();
        }
    }

    /// Returns the number of dirty addresses
    pub fn dirty_address_count(&self) -> usize {
        self.dirty_addresses.len()
    }

    /// Mark the first event as received, typically used on startup
    pub fn on_first_event(&mut self) {
        if self.first_event {
            self.set_state(PoolDriftState::Drifted);
            self.first_event = false;
        }
    }

    /// Add addresses to the set of dirty addresses
    pub fn add_dirty_addresses<I>(&mut self, addresses: I)
    where
        I: IntoIterator<Item = Address>,
    {
        self.dirty_addresses.extend(addresses);
    }

    /// Set the set of dirty addresses
    pub fn set_dirty_addresses(&mut self, addresses: HashSet<Address>) {
        self.dirty_addresses = addresses;
    }

    /// Take the set of dirty addresses, leaving the internal set empty
    pub fn take_dirty_addresses(&mut self) -> HashSet<Address> {
        std::mem::take(&mut self.dirty_addresses)
    }

    /// Remove an address from the set of dirty addresses
    pub fn remove_dirty_address(&mut self, address: &Address) -> bool {
        self.dirty_addresses.remove(address)
    }

    /// Returns `true` if the monitor has dirty addresses to reload
    pub fn has_dirty_addresses(&self) -> bool {
        !self.dirty_addresses.is_empty()
    }

    /// Start reloading accounts from state
    pub fn start_reload_accounts<Client, Spawner>(
        &mut self,
        client: Client,
        at: BlockHash,
        task_spawner: &Spawner,
    ) where
        Client: StateProviderFactory + Clone + 'static,
        Spawner: reth_tasks::TaskSpawner + 'static,
    {
        if !self.has_dirty_addresses() || !self.reload_accounts_fut.is_terminated() {
            return;
        }

        let (tx, rx) = oneshot::channel();

        let fut = if self.dirty_addresses.len() > self.max_reload_accounts {
            // need to chunk accounts to reload
            let accs_to_reload = self
                .dirty_addresses
                .iter()
                .copied()
                .take(self.max_reload_accounts)
                .collect::<Vec<_>>();

            for acc in &accs_to_reload {
                // make sure we remove them from the dirty set
                self.dirty_addresses.remove(acc);
            }

            async move {
                let res = load_accounts(client, at, accs_to_reload);
                let _ = tx.send(res);
            }
            .boxed()
        } else {
            // can fetch all dirty accounts at once
            let accs_to_reload = std::mem::take(&mut self.dirty_addresses);

            async move {
                let res = load_accounts(client, at, accs_to_reload);
                let _ = tx.send(res);
            }
            .boxed()
        };

        self.reload_accounts_fut = rx.fuse();
        task_spawner.spawn_blocking(fut);
    }

    /// update metrics based on the current state
    pub fn update_metrics(&self) {
        self.metrics.set_dirty_accounts_len(self.dirty_addresses.len());
    }
}

impl Future for DriftMonitor {
    type Output = DriftMonitorResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Check if reload future is ready
        if !self.reload_accounts_fut.is_terminated() {
            match self.reload_accounts_fut.poll_unpin(cx) {
                Poll::Ready(Ok(Ok(accounts))) => {
                    // reloaded accounts successfully
                    // extend accounts we failed to load from database
                    self.dirty_addresses.extend(accounts.failed_to_load.iter());
                    return Poll::Ready(DriftMonitorResult::AccountsLoaded(accounts));
                }
                Poll::Ready(Ok(Err(res))) => {
                    // failed to load accounts from state
                    let (accs, err) = *res;
                    debug!(target: "txpool", %err, "failed to load accounts");
                    self.dirty_addresses.extend(accs);
                    return Poll::Ready(DriftMonitorResult::Failed);
                }
                Poll::Ready(Err(_)) => {
                    // failed to receive the accounts, sender dropped, only possible if task
                    // panicked
                    self.set_state(PoolDriftState::Drifted);
                    return Poll::Ready(DriftMonitorResult::Failed);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        // no active reload operation - return Pending to avoid busy polling
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_drift_monitor_operations() {
        let metrics = MaintainPoolMetrics::default();
        let mut monitor = DriftMonitor::new(10, metrics);

        assert_eq!(monitor.state(), PoolDriftState::InSync);
        assert_eq!(monitor.dirty_address_count(), 0);
        assert!(!monitor.has_dirty_addresses());

        let addr1 = address!("0x1111111111111111111111111111111111111111");
        let addr2 = address!("0x2222222222222222222222222222222222222222");
        monitor.add_dirty_addresses([addr1, addr2]);
        assert_eq!(monitor.dirty_address_count(), 2);
        assert!(monitor.has_dirty_addresses());

        assert!(monitor.remove_dirty_address(&addr1));
        assert_eq!(monitor.dirty_address_count(), 1);

        monitor.set_state(PoolDriftState::Drifted);
        assert_eq!(monitor.state(), PoolDriftState::Drifted);
        assert!(monitor.state().is_drifted());

        monitor.set_state(PoolDriftState::InSync);
        assert_eq!(monitor.state(), PoolDriftState::InSync);
        assert!(!monitor.state().is_drifted());

        let addresses = monitor.take_dirty_addresses();
        assert_eq!(addresses.len(), 1);
        assert!(addresses.contains(&addr2));
        assert_eq!(monitor.dirty_address_count(), 0);
    }
}
