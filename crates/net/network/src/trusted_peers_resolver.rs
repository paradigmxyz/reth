//! Periodically resolves DNS records for a set of trusted peers and emits updates as they complete

use futures::{future::BoxFuture, ready, stream::FuturesUnordered, FutureExt, StreamExt};
use reth_network_peers::{NodeRecord, PeerId, TrustedPeer};
use std::{
    collections::HashMap,
    io,
    task::{Context, Poll},
};
use tokio::time::Interval;
use tracing::warn;

/// `TrustedPeersResolver` periodically spawns DNS resolution tasks for trusted peers.
/// It returns a resolved (`PeerId`, `NodeRecord`) update when one of its in‑flight tasks completes.
#[derive(Debug)]
pub struct TrustedPeersResolver {
    /// The trusted peers to resolve, keyed by [`PeerId`] for deduplication and O(1) removal.
    pub trusted_peers: HashMap<PeerId, TrustedPeer>,
    /// The timer that triggers a new resolution cycle.
    pub interval: Interval,
    /// Futures for currently in‑flight resolution tasks.
    pub pending: FuturesUnordered<BoxFuture<'static, (PeerId, Result<NodeRecord, io::Error>)>>,
}

impl TrustedPeersResolver {
    /// Create a new resolver with the given trusted peers and resolution interval.
    pub fn new(trusted_peers: Vec<TrustedPeer>, resolve_interval: Interval) -> Self {
        let trusted_peers = trusted_peers.into_iter().map(|p| (p.id, p)).collect();
        Self { trusted_peers, interval: resolve_interval, pending: FuturesUnordered::new() }
    }

    /// Update the resolution interval (useful for testing purposes)
    #[allow(dead_code)]
    pub fn set_interval(&mut self, interval: Interval) {
        self.interval = interval;
    }

    /// Poll the resolver.
    /// When the interval ticks, new resolution futures for each trusted peer are spawned.
    /// If a future completes successfully, it returns the resolved (`PeerId`, `NodeRecord`).
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<(PeerId, NodeRecord)> {
        if self.trusted_peers.is_empty() {
            return Poll::Pending;
        }

        if self.interval.poll_tick(cx).is_ready() {
            self.pending.clear();

            for trusted in self.trusted_peers.values().cloned() {
                let peer_id = trusted.id;
                let task = async move {
                    let result = trusted.resolve().await;
                    (peer_id, result)
                }
                .boxed();
                self.pending.push(task);
            }
        }

        match ready!(self.pending.poll_next_unpin(cx)) {
            Some((peer_id, Ok(record))) => Poll::Ready((peer_id, record)),
            Some((peer_id, Err(e))) => {
                warn!(target: "net::peers", "Failed to resolve trusted peer {:?}: {:?}", peer_id, e);
                Poll::Pending
            }
            None => Poll::Pending,
        }
    }
}
