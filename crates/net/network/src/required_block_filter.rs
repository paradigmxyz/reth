//! Required block peer filtering implementation.
//!
//! This module provides functionality to filter out peers that don't have
//! specific required blocks (primarily used for shadowfork testing).

use alloy_eips::BlockNumHash;
use futures::StreamExt;
use reth_eth_wire_types::{GetBlockHeaders, HeadersDirection};
use reth_network_api::{
    NetworkEvent, NetworkEventListenerProvider, PeerRequest, Peers, ReputationChangeKind,
};
use tokio::sync::oneshot;
use tracing::{debug, info, trace};

/// Task that filters peers based on required block hashes.
///
/// This task listens for new peer sessions and checks if they have the required
/// block hashes. Peers that don't have these blocks are banned.
///
/// This type is mainly used to connect peers on shadow forks (e.g. mainnet shadowfork)
pub struct RequiredBlockFilter<N> {
    /// Network handle for listening to events and managing peer reputation.
    network: N,
    /// List of block number-hash pairs that peers must have to be considered valid.
    block_num_hashes: Vec<BlockNumHash>,
}

impl<N> RequiredBlockFilter<N>
where
    N: NetworkEventListenerProvider + Peers + Clone + Send + Sync + 'static,
{
    /// Creates a new required block peer filter.
    pub const fn new(network: N, block_num_hashes: Vec<BlockNumHash>) -> Self {
        Self { network, block_num_hashes }
    }

    /// Spawns the required block peer filter task.
    ///
    /// This task will run indefinitely, monitoring new peer sessions and filtering
    /// out peers that don't have the required blocks.
    pub fn spawn(self) {
        if self.block_num_hashes.is_empty() {
            debug!(target: "net::filter", "No required block hashes configured, skipping peer filtering");
            return;
        }

        info!(target: "net::filter", "Starting required block peer filter with {} block hashes", self.block_num_hashes.len());

        tokio::spawn(async move {
            self.run().await;
        });
    }

    /// Main loop for the required block peer filter.
    async fn run(self) {
        let mut event_stream = self.network.event_listener();

        while let Some(event) = event_stream.next().await {
            if let NetworkEvent::ActivePeerSession { info, messages } = event {
                let peer_id = info.peer_id;
                debug!(target: "net::filter", "New peer session established: {}", peer_id);

                // Spawn a task to check this peer's blocks
                let network = self.network.clone();
                let block_num_hashes = self.block_num_hashes.clone();
                let peer_block_number = info.status.latest_block.unwrap_or(0);

                tokio::spawn(async move {
                    Self::check_peer_blocks(
                        network,
                        peer_id,
                        messages,
                        block_num_hashes,
                        peer_block_number,
                    )
                    .await;
                });
            }
        }
    }

    /// Checks if a peer has the required blocks and bans them if not.
    async fn check_peer_blocks(
        network: N,
        peer_id: reth_network_api::PeerId,
        messages: reth_network_api::PeerRequestSender<PeerRequest<N::Primitives>>,
        block_num_hashes: Vec<BlockNumHash>,
        latest_peer_block: u64,
    ) {
        for block_num_hash in block_num_hashes {
            // Skip if peer's block number is lower than required, peer might also be syncing and
            // still on the same chain.
            if block_num_hash.number > 0 && latest_peer_block < block_num_hash.number {
                debug!(target: "net::filter", "Skipping check for block {} - peer {} only at block {}", 
                       block_num_hash.number, peer_id, latest_peer_block);
                continue;
            }

            let block_hash = block_num_hash.hash;
            trace!(target: "net::filter", "Checking if peer {} has block {}", peer_id, block_hash);

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
                debug!(target: "net::filter", "Failed to send block header request to peer {}: {:?}", peer_id, e);
                continue;
            }

            // Wait for the response
            let response = match rx.await {
                Ok(response) => response,
                Err(e) => {
                    debug!(
                        target: "net::filter",
                        "Channel error getting block {} from peer {}: {:?}",
                        block_hash, peer_id, e
                    );
                    continue;
                }
            };

            let headers = match response {
                Ok(headers) => headers,
                Err(e) => {
                    debug!(target: "net::filter", "Error getting block {} from peer {}: {:?}", block_hash, peer_id, e);
                    // Ban the peer if they fail to respond properly
                    network.reputation_change(peer_id, ReputationChangeKind::BadProtocol);
                    return;
                }
            };

            if headers.is_empty() {
                info!(
                    target: "net::filter",
                    "Peer {} does not have required block {}, banning",
                    peer_id, block_hash
                );
                network.reputation_change(peer_id, ReputationChangeKind::BadProtocol);
                return; // No need to check more blocks if one is missing
            }

            trace!(target: "net::filter", "Peer {} has required block {}", peer_id, block_hash);
        }

        debug!(target: "net::filter", "Peer {} has all required blocks", peer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{b256, B256};
    use reth_network_api::noop::NoopNetwork;

    #[test]
    fn test_required_block_filter_creation() {
        let network = NoopNetwork::default();
        let block_num_hashes = vec![
            BlockNumHash::new(
                0,
                b256!("0x1111111111111111111111111111111111111111111111111111111111111111"),
            ),
            BlockNumHash::new(
                23115201,
                b256!("0x2222222222222222222222222222222222222222222222222222222222222222"),
            ),
        ];

        let filter = RequiredBlockFilter::new(network, block_num_hashes.clone());
        assert_eq!(filter.block_num_hashes.len(), 2);
        assert_eq!(filter.block_num_hashes, block_num_hashes);
    }

    #[test]
    fn test_required_block_filter_empty_hashes_does_not_spawn() {
        let network = NoopNetwork::default();
        let block_num_hashes = vec![];

        let filter = RequiredBlockFilter::new(network, block_num_hashes);
        // This should not panic and should exit early when spawn is called
        filter.spawn();
    }

    #[tokio::test]
    async fn test_required_block_filter_with_mock_peer() {
        // This test would require a more complex setup with mock network components
        // For now, we ensure the basic structure is correct
        let network = NoopNetwork::default();
        let block_num_hashes = vec![BlockNumHash::new(0, B256::default())];

        let filter = RequiredBlockFilter::new(network, block_num_hashes);
        // Verify the filter can be created and basic properties are set
        assert_eq!(filter.block_num_hashes.len(), 1);
    }
}
