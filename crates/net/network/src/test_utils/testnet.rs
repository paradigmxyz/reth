//! A network implementation for testing purposes.

use crate::{
    error::NetworkError, eth_requests::EthRequestHandler, NetworkConfig, NetworkConfigBuilder,
    NetworkEvent, NetworkHandle, NetworkManager,
};
use futures::{FutureExt, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::DisconnectReason;
use reth_primitives::PeerId;
use reth_provider::{test_utils::NoopProvider, BlockProvider, HeaderProvider};
use secp256k1::SecretKey;
use std::{
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

    /// Return a mutable slice of all peers.
    pub fn peers_mut(&mut self) -> &mut [Peer<C>] {
        &mut self.peers
    }

    /// Return a slice of all peers.
    pub fn peers(&self) -> &[Peer<C>] {
        &self.peers
    }

    /// Return a mutable iterator over all peers.
    pub fn peers_iter_mut(&mut self) -> impl Iterator<Item = &mut Peer<C>> + '_ {
        self.peers.iter_mut()
    }

    /// Return an iterator over all peers.
    pub fn peers_iter(&self) -> impl Iterator<Item = &Peer<C>> + '_ {
        self.peers.iter()
    }

    /// Extend the list of peers with new peers that are configured with each of the given
    /// [`PeerConfig`]s.
    pub async fn extend_peer_with_config(
        &mut self,
        configs: impl IntoIterator<Item = PeerConfig<C>>,
    ) -> Result<(), NetworkError> {
        for config in configs {
            self.add_peer_with_config(config).await?;
        }
        Ok(())
    }

    /// Add a peer to the [`Testnet`] with the given [`PeerConfig`].
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

impl Testnet<NoopProvider> {
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

    /// Add a peer to the [`Testnet`]
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

/// A handle to a [`Testnet`] that can be shared.
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
    /// Returns the number of connected peers.
    pub fn num_peers(&self) -> usize {
        self.network.num_connected_peers()
    }

    /// The address that listens for incoming connections.
    pub fn local_addr(&self) -> SocketAddr {
        self.network.local_addr()
    }

    /// Returns the [`NetworkHandle`] of this peer.
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

/// A helper config for setting up the reth networking stack.
pub struct PeerConfig<C = NoopProvider> {
    config: NetworkConfig<C>,
    client: Arc<C>,
    secret_key: SecretKey,
}

// === impl PeerConfig ===

impl<C> PeerConfig<C>
where
    C: BlockProvider + HeaderProvider,
{
    /// Initialize the network with a random secret key, allowing the devp2p and discovery to bind
    /// to any available IP and port.
    pub fn new(client: Arc<C>) -> Self {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        Self::with_secret_key(client, secret_key)
    }

    /// Initialize the network with a given secret key, allowing devp2p and discovery to bind any
    /// available IP and port.
    pub fn with_secret_key(client: Arc<C>, secret_key: SecretKey) -> Self {
        let config = NetworkConfigBuilder::new(secret_key)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .build(Arc::clone(&client));
        Self { config, client, secret_key }
    }
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self::new(Arc::new(NoopProvider::default()))
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
