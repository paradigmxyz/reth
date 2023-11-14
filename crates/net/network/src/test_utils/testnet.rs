//! A network implementation for testing purposes.

use crate::{
    builder::ETH_REQUEST_CHANNEL_CAPACITY,
    error::NetworkError,
    eth_requests::EthRequestHandler,
    transactions::{TransactionsHandle, TransactionsManager},
    NetworkConfig, NetworkConfigBuilder, NetworkEvent, NetworkEvents, NetworkHandle,
    NetworkManager,
};
use futures::{FutureExt, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::{protocol::Protocol, DisconnectReason, HelloMessageWithProtocols};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::{PeerId, MAINNET};
use reth_provider::{
    test_utils::NoopProvider, BlockReader, BlockReaderIdExt, HeaderProvider, StateProviderFactory,
};
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore,
    test_utils::{testing_pool, TestPool},
    EthTransactionPool, TransactionPool, TransactionValidationTaskExecutor,
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
    sync::{
        mpsc::{channel, unbounded_channel},
        oneshot,
    },
    task::JoinHandle,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A test network consisting of multiple peers.
pub struct Testnet<C, Pool> {
    /// All running peers in the network.
    peers: Vec<Peer<C, Pool>>,
}

// === impl Testnet ===

impl<C> Testnet<C, TestPool>
where
    C: BlockReader + HeaderProvider + Clone,
{
    /// Same as [`Self::try_create_with`] but panics on error
    pub async fn create_with(num_peers: usize, provider: C) -> Self {
        Self::try_create_with(num_peers, provider).await.unwrap()
    }

    /// Creates a new [`Testnet`] with the given number of peers and the provider.
    pub async fn try_create_with(num_peers: usize, provider: C) -> Result<Self, NetworkError> {
        let mut this = Self { peers: Vec::with_capacity(num_peers) };
        for _ in 0..num_peers {
            let config = PeerConfig::new(provider.clone());
            this.add_peer_with_config(config).await?;
        }
        Ok(this)
    }

    /// Extend the list of peers with new peers that are configured with each of the given
    /// [`PeerConfig`]s.
    pub async fn extend_peer_with_config(
        &mut self,
        configs: impl IntoIterator<Item = PeerConfig<C>>,
    ) -> Result<(), NetworkError> {
        let peers = configs.into_iter().map(|c| c.launch()).collect::<Vec<_>>();
        let peers = futures::future::join_all(peers).await;
        for peer in peers {
            self.peers.push(peer?);
        }
        Ok(())
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: BlockReader + HeaderProvider + Clone,
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

    /// Return a mutable iterator over all peers.
    pub fn peers_iter_mut(&mut self) -> impl Iterator<Item = &mut Peer<C, Pool>> + '_ {
        self.peers.iter_mut()
    }

    /// Return an iterator over all peers.
    pub fn peers_iter(&self) -> impl Iterator<Item = &Peer<C, Pool>> + '_ {
        self.peers.iter()
    }

    /// Add a peer to the [`Testnet`] with the given [`PeerConfig`].
    pub async fn add_peer_with_config(
        &mut self,
        config: PeerConfig<C>,
    ) -> Result<(), NetworkError> {
        let PeerConfig { config, client, secret_key } = config;

        let network = NetworkManager::new(config).await?;
        let peer = Peer {
            network,
            client,
            secret_key,
            request_handler: None,
            transactions_manager: None,
            pool: None,
        };
        self.peers.push(peer);
        Ok(())
    }

    /// Returns all handles to the networks
    pub fn handles(&self) -> impl Iterator<Item = NetworkHandle> + '_ {
        self.peers.iter().map(|p| p.handle())
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
    pub fn for_each<F>(&self, f: F)
    where
        F: Fn(&Peer<C, Pool>),
    {
        self.peers.iter().for_each(f)
    }

    /// Apply a closure on each peer
    pub fn for_each_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut Peer<C, Pool>),
    {
        self.peers.iter_mut().for_each(f)
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: StateProviderFactory + BlockReaderIdExt + HeaderProvider + Clone + 'static,
    Pool: TransactionPool,
{
    /// Installs an eth pool on each peer
    pub fn with_eth_pool(self) -> Testnet<C, EthTransactionPool<C, InMemoryBlobStore>> {
        self.map_pool(|peer| {
            let blob_store = InMemoryBlobStore::default();
            let pool = TransactionValidationTaskExecutor::eth(
                peer.client.clone(),
                MAINNET.clone(),
                blob_store.clone(),
                TokioTaskExecutor::default(),
            );
            peer.map_transactions_manager(EthTransactionPool::eth_pool(
                pool,
                blob_store,
                Default::default(),
            ))
        })
    }
}

impl<C, Pool> Testnet<C, Pool>
where
    C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
{
    /// Spawns the testnet to a separate task
    pub fn spawn(self) -> TestnetHandle<C, Pool> {
        let (tx, rx) = oneshot::channel::<oneshot::Sender<Self>>();
        let peers = self.peers.iter().map(|peer| peer.peer_handle()).collect::<Vec<_>>();
        let mut net = self;
        let handle = tokio::task::spawn(async move {
            let mut tx = None;
            tokio::select! {
                _ = &mut net => {}
                inc = rx => {
                    tx = inc.ok();
                }
            }
            if let Some(tx) = tx {
                let _ = tx.send(net);
            }
        });

        TestnetHandle { _handle: handle, peers, terminate: tx }
    }
}

impl Testnet<NoopProvider, TestPool> {
    /// Same as [`Self::try_create`] but panics on error
    pub async fn create(num_peers: usize) -> Self {
        Self::try_create(num_peers).await.unwrap()
    }

    /// Creates a new [`Testnet`] with the given number of peers
    pub async fn try_create(num_peers: usize) -> Result<Self, NetworkError> {
        let mut this = Testnet::default();

        this.extend_peer_with_config((0..num_peers).map(|_| Default::default())).await?;
        Ok(this)
    }

    /// Add a peer to the [`Testnet`]
    pub async fn add_peer(&mut self) -> Result<(), NetworkError> {
        self.add_peer_with_config(Default::default()).await
    }
}

impl<C, Pool> Default for Testnet<C, Pool> {
    fn default() -> Self {
        Self { peers: Vec::new() }
    }
}

impl<C, Pool> fmt::Debug for Testnet<C, Pool> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Testnet {{}}").finish_non_exhaustive()
    }
}

impl<C, Pool> Future for Testnet<C, Pool>
where
    C: BlockReader + HeaderProvider + Unpin,
    Pool: TransactionPool + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        for peer in this.peers.iter_mut() {
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

    /// Connects all peers with each other
    pub async fn connect_peers(&self) {
        if self.peers.len() < 2 {
            return
        }

        let mut streams = Vec::with_capacity(self.peers.len());
        let mut num_sessions = Vec::with_capacity(self.peers.len());
        for (idx, handle) in self.peers.iter().enumerate().take(self.peers.len() - 1) {
            streams.push(NetworkEventStream::new(handle.event_listener()));
            let mut num = 0;
            for idx in (idx + 1)..self.peers.len() {
                let neighbour = &self.peers[idx];
                handle.network.add_peer(*neighbour.peer_id(), neighbour.local_addr());
                num += 1;
            }
            num_sessions.push(num);
        }
        let fut = streams
            .into_iter()
            .zip(num_sessions)
            .map(|(mut stream, num)| async move { stream.take_session_established(num).await });
        futures::future::join_all(fut).await;
    }
}

/// A peer in the [`Testnet`].
#[pin_project]
#[derive(Debug)]
pub struct Peer<C, Pool = TestPool> {
    #[pin]
    network: NetworkManager<C>,
    #[pin]
    request_handler: Option<EthRequestHandler<C>>,
    #[pin]
    transactions_manager: Option<TransactionsManager<Pool>>,
    pool: Option<Pool>,
    client: C,
    secret_key: SecretKey,
}

// === impl Peer ===

impl<C, Pool> Peer<C, Pool>
where
    C: BlockReader + HeaderProvider + Clone,
    Pool: TransactionPool,
{
    /// Returns the number of connected peers.
    pub fn num_peers(&self) -> usize {
        self.network.num_connected_peers()
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
    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// Returns the [`NetworkHandle`] of this peer.
    pub fn handle(&self) -> NetworkHandle {
        self.network.handle().clone()
    }

    /// Returns the [`TestPool`] of this peer.
    pub fn pool(&self) -> Option<&Pool> {
        self.pool.as_ref()
    }

    /// Set a new request handler that's connected to the peer's network
    pub fn install_request_handler(&mut self) {
        let (tx, rx) = channel(ETH_REQUEST_CHANNEL_CAPACITY);
        self.network.set_eth_request_handler(tx);
        let peers = self.network.peers_handle();
        let request_handler = EthRequestHandler::new(self.client.clone(), peers, rx);
        self.request_handler = Some(request_handler);
    }

    /// Set a new transactions manager that's connected to the peer's network
    pub fn install_transactions_manager(&mut self, pool: Pool) {
        let (tx, rx) = unbounded_channel();
        self.network.set_transactions(tx);
        let transactions_manager = TransactionsManager::new(self.handle(), pool.clone(), rx);
        self.transactions_manager = Some(transactions_manager);
        self.pool = Some(pool);
    }

    /// Set a new transactions manager that's connected to the peer's network
    pub fn map_transactions_manager<P>(self, pool: P) -> Peer<C, P>
    where
        P: TransactionPool,
    {
        let Self { mut network, request_handler, client, secret_key, .. } = self;
        let (tx, rx) = unbounded_channel();
        network.set_transactions(tx);
        let transactions_manager =
            TransactionsManager::new(network.handle().clone(), pool.clone(), rx);
        Peer {
            network,
            request_handler,
            transactions_manager: Some(transactions_manager),
            pool: Some(pool),
            client,
            secret_key,
        }
    }
}

impl<C> Peer<C>
where
    C: BlockReader + HeaderProvider + Clone,
{
    /// Installs a new [testing_pool]
    pub fn install_test_pool(&mut self) {
        self.install_transactions_manager(testing_pool())
    }
}

impl<C, Pool> Future for Peer<C, Pool>
where
    C: BlockReader + HeaderProvider + Unpin,
    Pool: TransactionPool + Unpin + 'static,
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
#[derive(Debug)]
pub struct PeerConfig<C = NoopProvider> {
    config: NetworkConfig<C>,
    client: C,
    secret_key: SecretKey,
}

/// A handle to a peer in the [`Testnet`].
#[derive(Debug)]
pub struct PeerHandle<Pool> {
    network: NetworkHandle,
    transactions: Option<TransactionsHandle>,
    pool: Option<Pool>,
}

// === impl PeerHandle ===

impl<Pool> PeerHandle<Pool> {
    /// Returns the [`PeerId`] used in the network.
    pub fn peer_id(&self) -> &PeerId {
        self.network.peer_id()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// Creates a new [`NetworkEvent`] listener channel.
    pub fn event_listener(&self) -> UnboundedReceiverStream<NetworkEvent> {
        self.network.event_listener()
    }

    /// Returns the [`TransactionsHandle`] of this peer.
    pub fn transactions(&self) -> Option<&TransactionsHandle> {
        self.transactions.as_ref()
    }

    /// Returns the [`TestPool`] of this peer.
    pub fn pool(&self) -> Option<&Pool> {
        self.pool.as_ref()
    }

    /// Returns the [`NetworkHandle`] of this peer.
    pub fn network(&self) -> &NetworkHandle {
        &self.network
    }
}

// === impl PeerConfig ===

impl<C> PeerConfig<C>
where
    C: BlockReader + HeaderProvider + Clone,
{
    /// Launches the network and returns the [Peer] that manages it
    pub async fn launch(self) -> Result<Peer<C>, NetworkError> {
        let PeerConfig { config, client, secret_key } = self;
        let network = NetworkManager::new(config).await?;
        let peer = Peer {
            network,
            client,
            secret_key,
            request_handler: None,
            transactions_manager: None,
            pool: None,
        };
        Ok(peer)
    }

    /// Initialize the network with a random secret key, allowing the devp2p and discovery to bind
    /// to any available IP and port.
    pub fn new(client: C) -> Self {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let config = Self::network_config_builder(secret_key).build(client.clone());
        Self { config, client, secret_key }
    }

    /// Initialize the network with a given secret key, allowing devp2p and discovery to bind any
    /// available IP and port.
    pub fn with_secret_key(client: C, secret_key: SecretKey) -> Self {
        let config = Self::network_config_builder(secret_key).build(client.clone());
        Self { config, client, secret_key }
    }

    /// Initialize the network with a given capabilities.
    pub fn with_protocols(client: C, protocols: impl IntoIterator<Item = Protocol>) -> Self {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let builder = Self::network_config_builder(secret_key);
        let hello_message =
            HelloMessageWithProtocols::builder(builder.get_peer_id()).protocols(protocols).build();
        let config = builder.hello_message(hello_message).build(client.clone());

        Self { config, client, secret_key }
    }

    fn network_config_builder(secret_key: SecretKey) -> NetworkConfigBuilder {
        NetworkConfigBuilder::new(secret_key)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .disable_dns_discovery()
            .disable_discv4_discovery()
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
    inner: UnboundedReceiverStream<NetworkEvent>,
}

// === impl NetworkEventStream ===

impl NetworkEventStream {
    /// Create a new [`NetworkEventStream`] from the given network event receiver stream.
    pub fn new(inner: UnboundedReceiverStream<NetworkEvent>) -> Self {
        Self { inner }
    }

    /// Awaits the next event for a session to be closed
    pub async fn next_session_closed(&mut self) -> Option<(PeerId, Option<DisconnectReason>)> {
        while let Some(ev) = self.inner.next().await {
            match ev {
                NetworkEvent::SessionClosed { peer_id, reason } => return Some((peer_id, reason)),
                _ => continue,
            }
        }
        None
    }

    /// Awaits the next event for an established session
    pub async fn next_session_established(&mut self) -> Option<PeerId> {
        while let Some(ev) = self.inner.next().await {
            match ev {
                NetworkEvent::SessionEstablished { peer_id, .. } => return Some(peer_id),
                _ => continue,
            }
        }
        None
    }

    /// Awaits the next `num` events for an established session
    pub async fn take_session_established(&mut self, mut num: usize) -> Vec<PeerId> {
        if num == 0 {
            return Vec::new()
        }
        let mut peers = Vec::with_capacity(num);
        while let Some(ev) = self.inner.next().await {
            match ev {
                NetworkEvent::SessionEstablished { peer_id, .. } => {
                    peers.push(peer_id);
                    num -= 1;
                    if num == 0 {
                        return peers
                    }
                }
                _ => continue,
            }
        }
        peers
    }

    /// Ensures that the first two events are a [`NetworkEvent::PeerAdded`] and
    /// [`NetworkEvent::SessionEstablished`], returning the [`PeerId`] of the established
    /// session.
    pub async fn peer_added_and_established(&mut self) -> Option<PeerId> {
        let peer_id = match self.inner.next().await {
            Some(NetworkEvent::PeerAdded(peer_id)) => peer_id,
            _ => return None,
        };

        match self.inner.next().await {
            Some(NetworkEvent::SessionEstablished { peer_id: peer_id2, .. }) => {
                debug_assert_eq!(peer_id, peer_id2, "PeerAdded peer_id {peer_id} does not match SessionEstablished peer_id {peer_id2}");
                Some(peer_id)
            }
            _ => None,
        }
    }
}
