//! A network implementation for testing purposes.

use crate::{
    builder::ETH_REQUEST_CHANNEL_CAPACITY,
    config::rng_secret_key,
    error::NetworkError,
    eth_requests::EthRequestHandler,
    protocol::IntoRlpxSubProtocol,
    transactions::{
        config::{StrictEthAnnouncementFilter, TransactionPropagationKind},
        constants::tx_manager::DEFAULT_TX_MANAGER_CHANNEL_MEMORY_LIMIT_BYTES,
        policy::NetworkPolicies,
        TransactionsHandle, TransactionsManager, TransactionsManagerConfig,
    },
    NetworkConfigBuilder, NetworkHandle, NetworkManager, PeersConfig,
};
use futures::{FutureExt, StreamExt};
use pin_project::pin_project;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks, Hardforks};
use reth_eth_wire::{
    protocol::Protocol, DisconnectReason, EthNetworkPrimitives, HelloMessageWithProtocols,
};
use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
use reth_evm_ethereum::EthEvmConfig;
use reth_metrics::common::mpsc::memory_bounded_channel;
use reth_network_api::{
    events::{PeerEvent, SessionInfo},
    test_utils::{PeersHandle, PeersHandleProvider},
    NetworkEvent, NetworkEventListenerProvider, NetworkInfo, PeerRequest, Peers,
};
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_storage_api::{
    noop::NoopProvider, BalProvider, BlockNumReader, BlockReader, BlockReaderIdExt, HeaderProvider,
    StateProviderFactory, StateRangeProviderFactory,
};
use reth_tasks::Runtime;
use reth_tokio_util::EventStream;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, test_utils::TestPool, EthTransactionPool, PoolTransaction,
    TransactionPool, TransactionValidationTaskExecutor,
};
use secp256k1::SecretKey;
use std::{
    fmt,
    future::Future,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    sync::{mpsc::channel, oneshot},
    task::JoinHandle,
};

/// A test network consisting of multiple peers.
pub struct Testnet<C, Pool> {
    /// All running peers in the network.
    peers: Vec<Peer<C, Pool>>,
}

// === impl Testnet ===

impl<C> Testnet<C, TestPool>
where
    C: BlockNumReader + ChainSpecProvider<ChainSpec: Hardforks> + Clone + 'static,
{
    /// Creates a new [`Testnet`] with a peer for each of the given [`PeerConfig`]s.
    ///
    /// The peers are launched concurrently.
    ///
    /// # Panics
    ///
    /// If a peer fails to launch.
    pub async fn from_configs(configs: impl IntoIterator<Item = PeerConfig<C>>) -> Self {
        let peers = futures::future::try_join_all(configs.into_iter().map(PeerConfig::launch))
            .await
            .expect("failed to launch testnet peers");
        Self { peers }
    }

    /// Creates a new [`Testnet`] with the given number of peers that serve data from `provider`.
    ///
    /// # Panics
    ///
    /// If a peer fails to launch.
    pub async fn create_with(num_peers: usize, provider: C) -> Self {
        Self::from_configs((0..num_peers).map(|_| PeerConfig::new(provider.clone()))).await
    }

    /// Add a peer to the [`Testnet`] with the given [`PeerConfig`].
    pub async fn add_peer_with_config(
        &mut self,
        config: PeerConfig<C>,
    ) -> Result<(), NetworkError> {
        self.peers.push(config.launch().await?);
        Ok(())
    }
}

impl Testnet<NoopProvider, TestPool> {
    /// Creates a new [`Testnet`] with the given number of peers.
    ///
    /// # Panics
    ///
    /// If a peer fails to launch.
    pub async fn create(num_peers: usize) -> Self {
        Self::create_with(num_peers, NoopProvider::default()).await
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: BlockReader + HeaderProvider + Clone + 'static,
    Pool: TransactionPool,
{
    /// Return a mutable slice of all peers.
    pub fn peers_mut(&mut self) -> &mut [Peer<C, Pool>] {
        &mut self.peers
    }

    /// Return a slice of all peers.
    pub fn peers(&self) -> &[Peer<C, Pool>] {
        &self.peers
    }

    /// Maps the pool of each peer with the given closure
    pub fn map_pool<F, P>(self, f: F) -> Testnet<C, P>
    where
        F: Fn(Peer<C, Pool>) -> Peer<C, P>,
        P: TransactionPool,
    {
        Testnet { peers: self.peers.into_iter().map(f).collect() }
    }

    /// Apply a closure on each peer
    pub fn for_each_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut Peer<C, Pool>),
    {
        self.peers.iter_mut().for_each(f)
    }

    /// Installs an eth request handler on each peer.
    pub fn with_request_handlers(mut self) -> Self
    where
        C: BalProvider,
    {
        self.for_each_mut(Peer::install_request_handler);
        self
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + BlockReaderIdExt
        + HeaderProvider<Header = alloy_consensus::Header>
        + Clone
        + 'static,
    Pool: TransactionPool,
{
    /// Installs an eth pool on each peer
    pub fn with_eth_pool(
        self,
    ) -> Testnet<C, EthTransactionPool<C, InMemoryBlobStore, EthEvmConfig>> {
        self.with_eth_pool_config(Default::default())
    }

    /// Installs an eth pool on each peer with custom transaction manager config
    pub fn with_eth_pool_config(
        self,
        tx_manager_config: TransactionsManagerConfig,
    ) -> Testnet<C, EthTransactionPool<C, InMemoryBlobStore, EthEvmConfig>> {
        self.with_eth_pool_config_and_policy(tx_manager_config, Default::default())
    }

    /// Installs an eth pool on each peer with custom transaction manager config and policy.
    pub fn with_eth_pool_config_and_policy(
        self,
        tx_manager_config: TransactionsManagerConfig,
        policy: TransactionPropagationKind,
    ) -> Testnet<C, EthTransactionPool<C, InMemoryBlobStore, EthEvmConfig>> {
        self.map_pool(|peer| {
            let blob_store = InMemoryBlobStore::default();
            let validator = TransactionValidationTaskExecutor::eth(
                peer.client.clone(),
                EthEvmConfig::mainnet(),
                blob_store.clone(),
                Runtime::test(),
            );
            let pool = EthTransactionPool::eth_pool(validator, blob_store, Default::default());
            peer.map_transactions_manager(pool, tx_manager_config.clone(), policy)
        })
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: TestnetProvider + Clone,
    Pool: TestnetPool,
{
    /// Spawns the testnet to a separate task
    pub fn spawn(self) -> TestnetHandle<C, Pool> {
        let (terminate, rx) = oneshot::channel::<oneshot::Sender<Self>>();
        let peers = self.peers.iter().map(Peer::peer_handle).collect();
        let mut net = self;
        let handle = tokio::task::spawn(async move {
            let tx = tokio::select! {
                _ = &mut net => None,
                tx = rx => tx.ok(),
            };
            if let Some(tx) = tx {
                let _ = tx.send(net);
            }
        });

        TestnetHandle { _handle: handle, peers, terminate }
    }
}

impl<C, Pool> fmt::Debug for Testnet<C, Pool> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Testnet").finish_non_exhaustive()
    }
}

impl<C, Pool> Future for Testnet<C, Pool>
where
    C: TestnetProvider,
    Pool: TestnetPool,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        for peer in &mut this.peers {
            let _ = peer.poll_unpin(cx);
        }
        Poll::Pending
    }
}

/// A handle to a [`Testnet`] that can be shared.
#[derive(Debug)]
pub struct TestnetHandle<C, Pool> {
    _handle: JoinHandle<()>,
    peers: Vec<PeerHandle<Pool>>,
    terminate: oneshot::Sender<oneshot::Sender<Testnet<C, Pool>>>,
}

// === impl TestnetHandle ===

impl<C, Pool> TestnetHandle<C, Pool> {
    /// Terminates the task and returns the [`Testnet`] back.
    pub async fn terminate(self) -> Testnet<C, Pool> {
        let (tx, rx) = oneshot::channel();
        self.terminate.send(tx).unwrap();
        rx.await.unwrap()
    }

    /// Returns the [`PeerHandle`]s of this [`Testnet`].
    pub fn peers(&self) -> &[PeerHandle<Pool>] {
        &self.peers
    }

    /// Returns the [`PeerHandle`]s of this [`Testnet`] as an array, so they can be destructured:
    /// `let [peer0, peer1] = net.peers_array();`.
    ///
    /// # Panics
    ///
    /// If the testnet does not have exactly `N` peers.
    pub fn peers_array<const N: usize>(&self) -> &[PeerHandle<Pool>; N] {
        self.peers.as_slice().try_into().unwrap_or_else(|_| {
            panic!("expected {N} peers, but the testnet has {}", self.peers.len())
        })
    }

    /// Connects all peers with each other.
    ///
    /// This establishes sessions concurrently between all peers.
    ///
    /// Returns once all sessions are established.
    pub async fn connect_peers(&self) {
        if self.peers.len() < 2 {
            return
        }

        // add an event stream for _each_ peer
        let streams = self.peers.iter().map(PeerHandle::event_stream).collect::<Vec<_>>();

        // add all peers to each other
        for (idx, handle) in self.peers.iter().enumerate().take(self.peers.len() - 1) {
            for neighbour in &self.peers[idx + 1..] {
                handle.add_peer(neighbour);
            }
        }

        // await all sessions to be established
        let num_sessions_per_peer = self.peers.len() - 1;
        let fut = streams.into_iter().map(|mut stream| async move {
            stream.take_session_established(num_sessions_per_peer).await
        });

        futures::future::join_all(fut).await;
    }
}

/// A peer in the [`Testnet`].
#[pin_project]
#[derive(Debug)]
pub struct Peer<C, Pool = TestPool> {
    #[pin]
    network: NetworkManager<EthNetworkPrimitives>,
    #[pin]
    request_handler: Option<EthRequestHandler<C, EthNetworkPrimitives>>,
    #[pin]
    transactions_manager: Option<TransactionsManager<Pool, EthNetworkPrimitives>>,
    pool: Option<Pool>,
    client: C,
}

// === impl Peer ===

impl<C, Pool> Peer<C, Pool>
where
    C: BlockReader + HeaderProvider + Clone + 'static,
    Pool: TransactionPool,
{
    /// Returns the number of connected peers.
    pub fn num_peers(&self) -> usize {
        self.network.num_connected_peers()
    }

    /// Adds an additional protocol handler to the peer.
    pub fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        self.network.add_rlpx_sub_protocol(protocol);
    }

    /// Returns a handle to the peer's network.
    pub fn peer_handle(&self) -> PeerHandle<Pool> {
        PeerHandle {
            network: self.network.handle().clone(),
            pool: self.pool.clone(),
            transactions: self.transactions_manager.as_ref().map(|mgr| mgr.handle()),
        }
    }

    /// The address that listens for incoming connections.
    pub const fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// The [`PeerId`] of this peer.
    pub fn peer_id(&self) -> PeerId {
        *self.network.peer_id()
    }

    /// Returns mutable access to the network.
    pub const fn network_mut(&mut self) -> &mut NetworkManager<EthNetworkPrimitives> {
        &mut self.network
    }

    /// Returns the [`NetworkHandle`] of this peer.
    pub fn handle(&self) -> NetworkHandle<EthNetworkPrimitives> {
        self.network.handle().clone()
    }

    /// Returns the [`TestPool`] of this peer.
    pub const fn pool(&self) -> Option<&Pool> {
        self.pool.as_ref()
    }

    /// Set a new request handler that's connected to the peer's network
    pub fn install_request_handler(&mut self)
    where
        C: BalProvider,
    {
        let (tx, rx) = channel(ETH_REQUEST_CHANNEL_CAPACITY);
        self.network.set_eth_request_handler(tx);
        let peers = self.network.peers_handle();
        let request_handler = EthRequestHandler::new(self.client.clone(), peers, rx);
        self.request_handler = Some(request_handler);
    }

    /// Set a new transactions manager with the default config that's connected to the peer's
    /// network.
    pub fn install_transactions_manager(&mut self, pool: Pool) {
        self.transactions_manager = Some(new_transactions_manager(
            &mut self.network,
            pool.clone(),
            Default::default(),
            Default::default(),
        ));
        self.pool = Some(pool);
    }

    /// Replaces the pool with `pool` and connects a new transactions manager with the given config
    /// and propagation policy to the peer's network.
    pub fn map_transactions_manager<P>(
        self,
        pool: P,
        config: TransactionsManagerConfig,
        policy: TransactionPropagationKind,
    ) -> Peer<C, P>
    where
        P: TransactionPool,
    {
        let Self { mut network, request_handler, client, .. } = self;
        let transactions_manager =
            new_transactions_manager(&mut network, pool.clone(), config, policy);
        Peer {
            network,
            request_handler,
            transactions_manager: Some(transactions_manager),
            pool: Some(pool),
            client,
        }
    }
}

impl<C, Pool> Future for Peer<C, Pool>
where
    C: TestnetProvider,
    Pool: TestnetPool,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Some(request) = this.request_handler.as_pin_mut() {
            let _ = request.poll(cx);
        }

        if let Some(tx_manager) = this.transactions_manager.as_pin_mut() {
            let _ = tx_manager.poll(cx);
        }

        this.network.poll(cx)
    }
}

/// A helper config for setting up the reth networking stack.
///
/// The network config is built when the peer is launched.
#[derive(Debug)]
pub struct PeerConfig<C = NoopProvider> {
    client: C,
    secret_key: SecretKey,
    protocols: Option<Vec<Protocol>>,
    peers_config: PeersConfig,
}

/// A handle to a peer in the [`Testnet`].
#[derive(Debug)]
pub struct PeerHandle<Pool> {
    network: NetworkHandle<EthNetworkPrimitives>,
    transactions: Option<TransactionsHandle<EthNetworkPrimitives>>,
    pool: Option<Pool>,
}

// === impl PeerHandle ===

impl<Pool> PeerHandle<Pool> {
    /// Returns the [`PeerId`] used in the network.
    pub fn peer_id(&self) -> &PeerId {
        self.network.peer_id()
    }

    /// Returns the [`PeersHandle`] from the network.
    pub fn peer_handle(&self) -> &PeersHandle {
        self.network.peers_handle()
    }

    /// Returns the local socket as configured for the network.
    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// Creates a new [`NetworkEvent`] listener channel.
    pub fn event_listener(&self) -> EventStream<NetworkEvent> {
        self.network.event_listener()
    }

    /// Returns the [`TransactionsHandle`] of this peer.
    pub const fn transactions(&self) -> Option<&TransactionsHandle> {
        self.transactions.as_ref()
    }

    /// Returns the [`TestPool`] of this peer.
    pub const fn pool(&self) -> Option<&Pool> {
        self.pool.as_ref()
    }

    /// Returns the [`NetworkHandle`] of this peer.
    pub const fn network(&self) -> &NetworkHandle<EthNetworkPrimitives> {
        &self.network
    }

    /// Adds `other` to the peer set, so this peer connects to it.
    pub fn add_peer<P>(&self, other: &PeerHandle<P>) {
        self.network.add_peer(*other.peer_id(), other.local_addr());
    }

    /// Adds `other` to the peer set as a trusted peer, so this peer connects to it.
    pub fn add_trusted_peer<P>(&self, other: &PeerHandle<P>) {
        self.network.add_trusted_peer(*other.peer_id(), other.local_addr());
    }

    /// Creates a new [`NetworkEventStream`] of this peer's network events.
    pub fn event_stream(&self) -> NetworkEventStream {
        NetworkEventStream::new(self.event_listener())
    }

    /// Sends the request created by `request` to `peer_id` and awaits the response.
    ///
    /// Returns [`RequestError::ChannelClosed`] if the response channel is dropped.
    pub async fn request<R>(
        &self,
        peer_id: PeerId,
        request: impl FnOnce(oneshot::Sender<RequestResult<R>>) -> PeerRequest,
    ) -> RequestResult<R> {
        let (tx, rx) = oneshot::channel();
        self.network.send_request(peer_id, request(tx));
        rx.await.unwrap_or(Err(RequestError::ChannelClosed))
    }
}

// === impl PeerConfig ===

impl<C> PeerConfig<C> {
    /// Creates a config for a peer with a random secret key that serves data from `client`.
    ///
    /// The peer's devp2p and discovery bind to any available port.
    pub fn new(client: C) -> Self {
        Self {
            client,
            secret_key: rng_secret_key(),
            protocols: None,
            peers_config: PeersConfig::test(),
        }
    }

    /// Sets the secret key, which determines the peer's [`PeerId`].
    pub const fn with_secret_key(mut self, secret_key: SecretKey) -> Self {
        self.secret_key = secret_key;
        self
    }

    /// Sets the protocols advertised in the hello message, e.g. the supported eth versions.
    pub fn with_protocols(mut self, protocols: impl IntoIterator<Item: Into<Protocol>>) -> Self {
        self.protocols = Some(protocols.into_iter().map(Into::into).collect());
        self
    }

    /// Sets the [`PeersConfig`], which defaults to [`PeersConfig::test`].
    pub fn with_peers_config(mut self, peers_config: PeersConfig) -> Self {
        self.peers_config = peers_config;
        self
    }

    /// Launches the network and returns the [`Peer`] that manages it.
    pub async fn launch<Pool>(self) -> Result<Peer<C, Pool>, NetworkError>
    where
        C: BlockNumReader + ChainSpecProvider<ChainSpec: Hardforks> + Clone + 'static,
    {
        let Self { client, secret_key, protocols, peers_config } = self;
        let mut builder = NetworkConfigBuilder::new(secret_key, Runtime::test())
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .disable_dns_discovery()
            .disable_discv4_discovery()
            .peer_config(peers_config);
        if let Some(protocols) = protocols {
            // `NetworkConfigBuilder::build` re-derives snap advertisement from `snap_enabled`,
            // which would otherwise silently strip a manually included `snap` capability.
            let snap_enabled = protocols.iter().any(|p| p.cap.name == Protocol::snap_2().cap.name);
            let hello_message = HelloMessageWithProtocols::builder(builder.get_peer_id())
                .protocols(protocols)
                .build();
            builder = builder.with_snap(snap_enabled).hello_message(hello_message);
        }

        let network = NetworkManager::new(builder.build(client.clone())).await?;
        Ok(Peer { network, client, request_handler: None, transactions_manager: None, pool: None })
    }
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self::new(NoopProvider::default())
    }
}

/// A helper type to await network events
///
/// This makes it easier to await established connections
#[derive(Debug)]
pub struct NetworkEventStream {
    inner: EventStream<NetworkEvent>,
}

// === impl NetworkEventStream ===

impl NetworkEventStream {
    /// Create a new [`NetworkEventStream`] from the given network event receiver stream.
    pub const fn new(inner: EventStream<NetworkEvent>) -> Self {
        Self { inner }
    }

    /// Awaits the next event for a session to be closed
    pub async fn next_session_closed(&mut self) -> Option<(PeerId, Option<DisconnectReason>)> {
        while let Some(ev) = self.inner.next().await {
            if let NetworkEvent::Peer(PeerEvent::SessionClosed { peer_id, reason }) = ev {
                return Some((peer_id, reason))
            }
        }
        None
    }

    /// Awaits the next event for an established session
    pub async fn next_session_established(&mut self) -> Option<PeerId> {
        while let Some(ev) = self.inner.next().await {
            match ev {
                NetworkEvent::ActivePeerSession { info, .. } |
                NetworkEvent::Peer(PeerEvent::SessionEstablished(info)) => {
                    return Some(info.peer_id)
                }
                _ => {}
            }
        }
        None
    }

    /// Awaits the next `num` events for an established session
    pub async fn take_session_established(&mut self, mut num: usize) -> Vec<PeerId> {
        if num == 0 {
            return Vec::new();
        }
        let mut peers = Vec::with_capacity(num);
        while let Some(ev) = self.inner.next().await {
            if let NetworkEvent::ActivePeerSession { info: SessionInfo { peer_id, .. }, .. } = ev {
                peers.push(peer_id);
                num -= 1;
                if num == 0 {
                    return peers;
                }
            }
        }
        peers
    }

    /// Returns the peer of the next event if it is a [`PeerEvent::PeerAdded`].
    pub async fn peer_added(&mut self) -> Option<PeerId> {
        match self.inner.next().await {
            Some(NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id))) => Some(peer_id),
            _ => None,
        }
    }

    /// Returns the peer of the next event if it is a [`PeerEvent::PeerRemoved`].
    pub async fn peer_removed(&mut self) -> Option<PeerId> {
        match self.inner.next().await {
            Some(NetworkEvent::Peer(PeerEvent::PeerRemoved(peer_id))) => Some(peer_id),
            _ => None,
        }
    }
}

/// A provider that can serve all eth requests of a [`Peer`] in a spawned [`Testnet`].
pub trait TestnetProvider:
    BlockReader<
        Block = reth_ethereum_primitives::Block,
        Receipt = reth_ethereum_primitives::Receipt,
        Header = alloy_consensus::Header,
    > + HeaderProvider
    + BalProvider
    + StateProviderFactory
    + StateRangeProviderFactory
    + Unpin
    + 'static
{
}

impl<T> TestnetProvider for T where
    T: BlockReader<
            Block = reth_ethereum_primitives::Block,
            Receipt = reth_ethereum_primitives::Receipt,
            Header = alloy_consensus::Header,
        > + HeaderProvider
        + BalProvider
        + StateProviderFactory
        + StateRangeProviderFactory
        + Unpin
        + 'static
{
}

/// A transaction pool that can back the transactions manager of a [`Peer`] in a spawned
/// [`Testnet`].
pub trait TestnetPool:
    TransactionPool<
        Transaction: PoolTransaction<
            Consensus = TransactionSigned,
            Pooled = PooledTransactionVariant,
        >,
    > + Unpin
    + 'static
{
}

impl<T> TestnetPool for T where
    T: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = TransactionSigned,
                Pooled = PooledTransactionVariant,
            >,
        > + Unpin
        + 'static
{
}

/// Creates a transactions manager for `pool` that is connected to `network`.
fn new_transactions_manager<Pool: TransactionPool>(
    network: &mut NetworkManager<EthNetworkPrimitives>,
    pool: Pool,
    config: TransactionsManagerConfig,
    policy: TransactionPropagationKind,
) -> TransactionsManager<Pool, EthNetworkPrimitives> {
    let (tx, rx) =
        memory_bounded_channel(DEFAULT_TX_MANAGER_CHANNEL_MEMORY_LIMIT_BYTES, "test_tx_channel");
    network.set_transactions(tx);
    let policies = NetworkPolicies::new(policy, StrictEthAnnouncementFilter::default());
    TransactionsManager::with_policy(network.handle().clone(), pool, rx, config, policies)
}
