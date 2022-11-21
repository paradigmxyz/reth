//! A network implementation for testing purposes.

use futures::FutureExt;
use pin_project::pin_project;
use reth_interfaces::provider::BlockProvider;
use reth_network::{NetworkConfig, NetworkHandle, NetworkManager};
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

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

    pub fn add_peer(&mut self) {
        self.add_peer_with_config(Default::default())
    }

    pub fn add_peer_with_config(&mut self, config: NetworkConfig<C>) {}
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

#[pin_project]
struct Peer<C> {
    #[pin]
    network: NetworkManager<C>,
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

#[derive(Default)]
struct PeerConfig {}
