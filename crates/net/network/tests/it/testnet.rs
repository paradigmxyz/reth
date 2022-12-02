//! A network implementation for testing purposes.

use futures::{FutureExt, StreamExt};
use parking_lot::Mutex;
use pin_project::pin_project;
use reth_interfaces::{
    provider::{BlockProvider, ChainInfo, HeaderProvider},
    test_utils::TestApi,
};
use reth_network::{
    error::NetworkError, eth_requests::EthRequestHandler, NetworkConfig, NetworkEvent,
    NetworkHandle, NetworkManager,
};
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, BlockHash, Header, PeerId, H256, U256,
};
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    sync::{mpsc::unbounded_channel, oneshot},
    task::JoinHandle,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A test network consisting of multiple peers.
#[derive(Default)]
pub struct Testnet<C> {
    /// All running peers in the network.
    peers: Vec<Peer<C>>,
}

// === impl Testnet ===

impl<C> Testnet<C>
where
    C: BlockProvider + HeaderProvider,
{
    /// Same as [`Self::try_create_with`] but panics on error
    pub async fn create_with(num_peers: usize, provider: Arc<C>) -> Self {
        Self::try_create_with(num_peers, provider).await.unwrap()
    }

    /// Creates a new [`Testnet`] with the given number of peers and the provider.
    pub async fn try_create_with(num_peers: usize, provider: Arc<C>) -> Result<Self, NetworkError> {
        let mut this = Self { peers: Vec::with_capacity(num_peers) };
        for _ in 0..num_peers {
            let config = PeerConfig::new(Arc::clone(&provider));
            this.add_peer_with_config(config).await?;
        }
        Ok(this)
    }

    pub fn peers_mut(&mut self) -> &mut [Peer<C>] {
        &mut self.peers
    }

    pub fn peers(&self) -> &[Peer<C>] {
        &self.peers
    }

    pub fn peers_iter_mut(&mut self) -> impl Iterator<Item = &mut Peer<C>> + '_ {
        self.peers.iter_mut()
    }

    pub fn peers_iter(&self) -> impl Iterator<Item = &Peer<C>> + '_ {
        self.peers.iter()
    }

    pub async fn add_peer_with_config(
        &mut self,
        config: PeerConfig<C>,
    ) -> Result<(), NetworkError> {
        let PeerConfig { config, client, secret_key } = config;

        let network = NetworkManager::new(config).await?;
        let peer = Peer { network, client, secret_key, request_handler: None };
        self.peers.push(peer);
        Ok(())
    }

    /// Returns all handles to the networks
    pub fn handles(&self) -> impl Iterator<Item = NetworkHandle> + '_ {
        self.peers.iter().map(|p| p.handle())
    }

    /// Apply a closure on each peer
    pub fn for_each<F>(&self, f: F)
    where
        F: Fn(&Peer<C>),
    {
        self.peers.iter().for_each(f)
    }

    /// Apply a closure on each peer
    pub fn for_each_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut Peer<C>),
    {
        self.peers.iter_mut().for_each(f)
    }
}

impl<C> Testnet<C>
where
    C: BlockProvider + HeaderProvider + 'static,
{
    /// Spawns the testnet to a separate task
    pub fn spawn(self) -> TestnetHandle<C> {
        let (tx, rx) = oneshot::channel::<oneshot::Sender<Self>>();
        let mut net = self;
        let handle = tokio::task::spawn(async move {
            let mut tx = None;
            loop {
                tokio::select! {
                    _ = &mut net => { break}
                    inc = rx => {
                        tx = inc.ok();
                        break
                    }
                }
            }
            if let Some(tx) = tx {
                let _ = tx.send(net);
            }
        });

        TestnetHandle { _handle: handle, terminate: tx }
    }
}

impl Testnet<TestApi> {
    /// Same as [`Self::try_create`] but panics on error
    pub async fn create(num_peers: usize) -> Self {
        Self::try_create(num_peers).await.unwrap()
    }

    /// Creates a new [`Testnet`] with the given number of peers
    pub async fn try_create(num_peers: usize) -> Result<Self, NetworkError> {
        let mut this = Testnet::default();
        for _ in 0..num_peers {
            this.add_peer().await?;
        }
        Ok(this)
    }

    pub async fn add_peer(&mut self) -> Result<(), NetworkError> {
        self.add_peer_with_config(Default::default()).await
    }
}

impl<C> fmt::Debug for Testnet<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Testnet {{}}").finish_non_exhaustive()
    }
}

impl<C> Future for Testnet<C>
where
    C: BlockProvider + HeaderProvider,
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

pub struct TestnetHandle<C> {
    _handle: JoinHandle<()>,
    terminate: oneshot::Sender<oneshot::Sender<Testnet<C>>>,
}

// === impl TestnetHandle ===

impl<C> TestnetHandle<C> {
    /// Terminates the task and returns the [`Testnet`] back.
    pub async fn terminate(self) -> Testnet<C> {
        let (tx, rx) = oneshot::channel();
        self.terminate.send(tx).unwrap();
        rx.await.unwrap()
    }
}

#[pin_project]
pub struct Peer<C> {
    #[pin]
    network: NetworkManager<C>,
    #[pin]
    request_handler: Option<EthRequestHandler<C>>,
    client: Arc<C>,
    secret_key: SecretKey,
}

// === impl Peer ===

impl<C> Peer<C>
where
    C: BlockProvider + HeaderProvider,
{
    pub fn num_peers(&self) -> usize {
        self.network.num_connected_peers()
    }

    /// The address that listens for incoming connections.
    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    pub fn handle(&self) -> NetworkHandle {
        self.network.handle().clone()
    }

    /// Set a new request handler that's connected tot the peer's network
    pub fn install_request_handler(&mut self) {
        let (tx, rx) = unbounded_channel();
        self.network.set_eth_request_handler(tx);
        let peers = self.network.peers_handle();
        let request_handler = EthRequestHandler::new(Arc::clone(&self.client), peers, rx);
        self.request_handler = Some(request_handler);
    }
}

impl<C> Future for Peer<C>
where
    C: BlockProvider + HeaderProvider,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Some(request) = this.request_handler.as_pin_mut() {
            let _ = request.poll(cx);
        }

        this.network.poll(cx)
    }
}

pub struct PeerConfig<C = TestApi> {
    config: NetworkConfig<C>,
    client: Arc<C>,
    secret_key: SecretKey,
}

// === impl PeerConfig ===

impl<C> PeerConfig<C>
where
    C: BlockProvider + HeaderProvider,
{
    pub fn new(client: Arc<C>) -> Self {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let config = NetworkConfig::builder(Arc::clone(&client), secret_key)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .build();
        Self { config, client, secret_key }
    }
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self::new(Arc::new(TestApi::default()))
    }
}

/// A helper type to await network events
///
/// This makes it easier to await established connections
pub struct NetworkEventStream {
    inner: UnboundedReceiverStream<NetworkEvent>,
}

// === impl NetworkEventStream ===

impl NetworkEventStream {
    pub fn new(inner: UnboundedReceiverStream<NetworkEvent>) -> Self {
        Self { inner }
    }

    /// Awaits the next event for an established session
    pub async fn next_session_established(&mut self) -> Option<PeerId> {
        while let Some(ev) = self.inner.next().await {
            match ev {
                NetworkEvent::SessionClosed { .. } => continue,
                NetworkEvent::SessionEstablished { peer_id, .. } => return Some(peer_id),
            }
        }
        None
    }
}

/// A mock implementation for Provider interfaces.
#[derive(Debug, Clone, Default)]
pub struct MockEthProvider {
    pub blocks: Arc<Mutex<HashMap<H256, Block>>>,
    pub headers: Arc<Mutex<HashMap<H256, Header>>>,
}

impl MockEthProvider {
    pub fn add_block(&self, hash: H256, block: Block) {
        self.blocks.lock().insert(hash, block);
    }

    pub fn extend_blocks(&self, iter: impl IntoIterator<Item = (H256, Block)>) {
        for (hash, block) in iter.into_iter() {
            self.add_block(hash, block)
        }
    }

    pub fn add_header(&self, hash: H256, header: Header) {
        self.headers.lock().insert(hash, header);
    }

    pub fn extend_headers(&self, iter: impl IntoIterator<Item = (H256, Header)>) {
        for (hash, header) in iter.into_iter() {
            self.add_header(hash, header)
        }
    }
}

impl HeaderProvider for MockEthProvider {
    fn header(&self, block_hash: &BlockHash) -> reth_interfaces::Result<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.get(block_hash).cloned())
    }

    fn header_by_number(&self, num: u64) -> reth_interfaces::Result<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.values().find(|h| h.number == num).cloned())
    }
}

impl BlockProvider for MockEthProvider {
    fn chain_info(&self) -> reth_interfaces::Result<ChainInfo> {
        todo!()
    }

    fn block(&self, id: BlockId) -> reth_interfaces::Result<Option<Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockId::Hash(hash) => Ok(lock.get(&hash).cloned()),
            BlockId::Number(BlockNumber::Number(num)) => {
                Ok(lock.values().find(|b| b.number == num.as_u64()).cloned())
            }
            _ => {
                unreachable!("unused in network tests")
            }
        }
    }

    fn block_number(
        &self,
        hash: H256,
    ) -> reth_interfaces::Result<Option<reth_primitives::BlockNumber>> {
        let lock = self.blocks.lock();
        let num = lock.iter().find_map(|(h, b)| if *h == hash { Some(b.number) } else { None });
        Ok(num)
    }

    fn block_hash(&self, number: U256) -> reth_interfaces::Result<Option<H256>> {
        let lock = self.blocks.lock();

        let hash =
            lock.iter().find_map(
                |(hash, b)| {
                    if b.number == number.as_u64() {
                        Some(*hash)
                    } else {
                        None
                    }
                },
            );
        Ok(hash)
    }
}
