//! A network implementation for testing purposes.

use futures::FutureExt;
use pin_project::pin_project;
use reth_interfaces::{provider::BlockProvider, test_utils::TestApi};
use reth_network::{error::NetworkError, NetworkConfig, NetworkHandle, NetworkManager};
use secp256k1::SecretKey;
use std::{
    fmt,
    future::Future,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{sync::oneshot, task::JoinHandle};

/// A test network consisting of multiple peers.
#[derive(Default)]
pub struct Testnet<C> {
    /// All running peers in the network.
    peers: Vec<Peer<C>>,
}

// === impl Testnet ===

impl<C> Testnet<C>
where
    C: BlockProvider,
{
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
        let peer = Peer { network, client, secret_key };
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
}

impl<C> Testnet<C>
where
    C: BlockProvider + 'static,
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
    C: BlockProvider,
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
    client: Arc<C>,
    secret_key: SecretKey,
}

// === impl Peer ===

impl<C> Peer<C>
where
    C: BlockProvider,
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
}

impl<C> Future for Peer<C>
where
    C: BlockProvider,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().network.poll(cx)
    }
}

pub struct PeerConfig<C = TestApi> {
    config: NetworkConfig<C>,
    client: Arc<C>,
    secret_key: SecretKey,
}

impl Default for PeerConfig {
    fn default() -> Self {
        let client = Arc::new(TestApi::default());
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let config = NetworkConfig::builder(Arc::clone(&client), secret_key)
            .listener_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .build();
        Self { config, client, secret_key }
    }
}
