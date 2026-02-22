//! Builder support for configuring the entire setup.

use crate::{
    eth_requests::EthRequestHandler,
    transactions::{
        config::{
            AnnouncementFilteringPolicy, StrictEthAnnouncementFilter, TransactionPropagationKind,
        },
        policy::NetworkPolicies,
        TransactionPropagationPolicy, TransactionsManager, TransactionsManagerConfig,
    },
    NetworkHandle, NetworkManager,
};
use reth_bal_store::{BalStore, NoopBalStore};
use reth_eth_wire::{EthNetworkPrimitives, NetworkPrimitives};
use reth_network_api::test_utils::PeersHandleProvider;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;
use tokio::sync::mpsc;

/// We set the max channel capacity of the `EthRequestHandler` to 256
/// 256 requests with malicious 10MB body requests is 2.6GB which can be absorbed by the node.
pub(crate) const ETH_REQUEST_CHANNEL_CAPACITY: usize = 256;

/// A builder that can configure all components of the network.
#[expect(missing_debug_implementations)]
pub struct NetworkBuilder<Tx, Eth, N: NetworkPrimitives = EthNetworkPrimitives> {
    pub(crate) network: NetworkManager<N>,
    pub(crate) transactions: Tx,
    pub(crate) request_handler: Eth,
}

// === impl NetworkBuilder ===

impl<Tx, Eth, N: NetworkPrimitives> NetworkBuilder<Tx, Eth, N> {
    /// Consumes the type and returns all fields.
    pub fn split(self) -> (NetworkManager<N>, Tx, Eth) {
        let Self { network, transactions, request_handler } = self;
        (network, transactions, request_handler)
    }

    /// Returns the network manager.
    pub const fn network(&self) -> &NetworkManager<N> {
        &self.network
    }

    /// Returns the mutable network manager.
    pub const fn network_mut(&mut self) -> &mut NetworkManager<N> {
        &mut self.network
    }

    /// Returns the handle to the network.
    pub fn handle(&self) -> NetworkHandle<N> {
        self.network.handle().clone()
    }

    /// Consumes the type and returns all fields and also return a [`NetworkHandle`].
    pub fn split_with_handle(self) -> (NetworkHandle<N>, NetworkManager<N>, Tx, Eth) {
        let Self { network, transactions, request_handler } = self;
        let handle = network.handle().clone();
        (handle, network, transactions, request_handler)
    }

    /// Creates a new [`EthRequestHandler`] and wires it to the network.
    pub fn request_handler<Client>(
        self,
        client: Client,
    ) -> NetworkBuilder<Tx, EthRequestHandler<Client, N>, N> {
        self.request_handler_with_bal_store(client, Arc::new(NoopBalStore))
    }

    /// Creates a new [`EthRequestHandler`] with a custom BAL store and wires it to the network.
    pub fn request_handler_with_bal_store<Client>(
        self,
        client: Client,
        bal_store: Arc<dyn BalStore>,
    ) -> NetworkBuilder<Tx, EthRequestHandler<Client, N>, N> {
        let Self { mut network, transactions, .. } = self;
        let (tx, rx) = mpsc::channel(ETH_REQUEST_CHANNEL_CAPACITY);
        network.set_eth_request_handler(tx);
        let peers = network.handle().peers_handle().clone();
        let request_handler = EthRequestHandler::with_bal_store(client, peers, rx, bal_store);
        NetworkBuilder { network, request_handler, transactions }
    }

    /// Creates a new [`TransactionsManager`] and wires it to the network.
    pub fn transactions<Pool: TransactionPool>(
        self,
        pool: Pool,
        transactions_manager_config: TransactionsManagerConfig,
    ) -> NetworkBuilder<TransactionsManager<Pool, N>, Eth, N> {
        self.transactions_with_policy(
            pool,
            transactions_manager_config,
            TransactionPropagationKind::default(),
        )
    }

    /// Creates a new [`TransactionsManager`] and wires it to the network.
    ///
    /// Uses the default [`StrictEthAnnouncementFilter`] for announcement filtering.
    pub fn transactions_with_policy<Pool: TransactionPool, P: TransactionPropagationPolicy<N>>(
        self,
        pool: Pool,
        transactions_manager_config: TransactionsManagerConfig,
        propagation_policy: P,
    ) -> NetworkBuilder<TransactionsManager<Pool, N>, Eth, N> {
        self.transactions_with_policies(
            pool,
            transactions_manager_config,
            propagation_policy,
            StrictEthAnnouncementFilter::default(),
        )
    }

    /// Creates a new [`TransactionsManager`] with custom propagation and announcement policies.
    ///
    /// This allows chains with custom transaction types (like CATX) to configure
    /// the announcement filter to accept their transaction types.
    pub fn transactions_with_policies<
        Pool: TransactionPool,
        P: TransactionPropagationPolicy<N>,
        A: AnnouncementFilteringPolicy<N>,
    >(
        self,
        pool: Pool,
        transactions_manager_config: TransactionsManagerConfig,
        propagation_policy: P,
        announcement_policy: A,
    ) -> NetworkBuilder<TransactionsManager<Pool, N>, Eth, N> {
        let Self { mut network, request_handler, .. } = self;
        let (tx, rx) = mpsc::unbounded_channel();
        network.set_transactions(tx);
        let handle = network.handle().clone();
        let policies = NetworkPolicies::new(propagation_policy, announcement_policy);

        let transactions = TransactionsManager::with_policy(
            handle,
            pool,
            rx,
            transactions_manager_config,
            policies,
        );
        NetworkBuilder { network, request_handler, transactions }
    }
}
