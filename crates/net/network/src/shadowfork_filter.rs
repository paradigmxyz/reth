//! Shadowfork peer filtering implementation.
//!
//! This module provides functionality to filter out peers that are not part of
//! a shadowfork by checking if they have specific block hashes.

use alloy_primitives::B256;
use futures::StreamExt;
use reth_eth_wire_types::{GetBlockHeaders, HeadersDirection};
use reth_network_api::{
    NetworkEvent, NetworkEventListenerProvider, PeerRequest, Peers, ReputationChangeKind,
};
use tokio::sync::oneshot;
use tracing::{debug, info, trace};

/// Task that filters peers based on shadowfork block hashes.
///
/// This task listens for new peer sessions and checks if they have the required
/// block hashes. Peers that don't have these blocks are banned.
pub struct ShadowforkPeerFilter<N> {
    /// Network handle for listening to events and managing peer reputation.
    network: N,
    /// List of block hashes that peers must have to be considered valid.
    block_hashes: Vec<B256>,
}

impl<N> ShadowforkPeerFilter<N>
where
    N: NetworkEventListenerProvider + Peers + Clone + Send + Sync + 'static,
{
    /// Creates a new shadowfork peer filter.
    pub const fn new(network: N, block_hashes: Vec<B256>) -> Self {
        Self { network, block_hashes }
    }

    /// Spawns the shadowfork peer filter task.
    ///
    /// This task will run indefinitely, monitoring new peer sessions and filtering
    /// out peers that don't have the required blocks.
    pub fn spawn(self) {
        if self.block_hashes.is_empty() {
            debug!("No shadowfork block hashes configured, skipping peer filtering");
            return;
        }

        info!("Starting shadowfork peer filter with {} block hashes", self.block_hashes.len());

        tokio::spawn(async move {
            self.run().await;
        });
    }

    /// Main loop for the shadowfork peer filter.
    async fn run(self) {
        let mut event_stream = self.network.event_listener();

        while let Some(event) = event_stream.next().await {
            if let NetworkEvent::ActivePeerSession { info, messages } = event {
                    let peer_id = info.peer_id;
                    debug!("New peer session established: {}", peer_id);

                    // Spawn a task to check this peer's blocks
                    let network = self.network.clone();
                    let block_hashes = self.block_hashes.clone();

                    tokio::spawn(async move {
                    Self::check_peer_blocks(network, peer_id, messages, block_hashes).await;
                });
            }
        }
    }

    /// Checks if a peer has the required blocks and bans them if not.
    async fn check_peer_blocks(
        network: N,
        peer_id: reth_network_api::PeerId,
        messages: reth_network_api::PeerRequestSender<PeerRequest<N::Primitives>>,
        block_hashes: Vec<B256>,
    ) {
        for block_hash in block_hashes {
            trace!("Checking if peer {} has block {}", peer_id, block_hash);

            // Create a request for block headers
            let request = GetBlockHeaders {
                start_block: block_hash.into(),
                limit: 1,
                skip: 0,
                direction: HeadersDirection::Rising,
            };

            let (tx, rx) = oneshot::channel();
            let peer_request = PeerRequest::GetBlockHeaders { request, response: tx };

            // Send the request to the peer
            if let Err(e) = messages.try_send(peer_request) {
                debug!("Failed to send block header request to peer {}: {:?}", peer_id, e);
                continue;
            }

            // Wait for the response
            match rx.await {
                Ok(Ok(headers)) => {
                    if headers.0.is_empty() {
                        info!(
                            "Peer {} does not have required block {}, banning",
                            peer_id, block_hash
                        );
                        network.reputation_change(peer_id, ReputationChangeKind::BadProtocol);
                        return; // No need to check more blocks if one is missing
                    } 
                    trace!("Peer {} has required block {}", peer_id, block_hash);
                }
                Ok(Err(e)) => {
                    debug!("Error getting block {} from peer {}: {:?}", block_hash, peer_id, e);
                    // Ban the peer if they fail to respond properly
                    network.reputation_change(peer_id, ReputationChangeKind::BadProtocol);
                    return;
                }
                Err(e) => {
                    debug!(
                        "Channel error getting block {} from peer {}: {:?}",
                        block_hash, peer_id, e
                    );
                }
            }
        }

        debug!("Peer {} has all required shadowfork blocks", peer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_network_api::noop::NoopNetwork;
    use std::str::FromStr;

    #[test]
    fn test_shadowfork_filter_creation() {
        let network = NoopNetwork::default();
        let block_hashes = vec![
            B256::from_str("0x1111111111111111111111111111111111111111111111111111111111111111")
                .unwrap(),
            B256::from_str("0x2222222222222222222222222222222222222222222222222222222222222222")
                .unwrap(),
        ];

        let filter = ShadowforkPeerFilter::new(network, block_hashes.clone());
        assert_eq!(filter.block_hashes.len(), 2);
        assert_eq!(filter.block_hashes, block_hashes);
    }

    #[test]
    fn test_shadowfork_filter_empty_hashes_does_not_spawn() {
        let network = NoopNetwork::default();
        let block_hashes = vec![];

        let filter = ShadowforkPeerFilter::new(network, block_hashes);
        // This should not panic and should exit early when spawn is called
        filter.spawn();
    }

    #[tokio::test]
    async fn test_shadowfork_filter_with_mock_peer() {
        // This test would require a more complex setup with mock network components
        // For now, we ensure the basic structure is correct
        let network = NoopNetwork::default();
        let block_hashes = vec![B256::default()];

        let filter = ShadowforkPeerFilter::new(network.clone(), block_hashes);
        // Verify the filter can be created and basic properties are set
        assert_eq!(filter.block_hashes.len(), 1);
    }
}
