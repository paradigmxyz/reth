use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::BlockNumber;
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_primitives::{NodePrimitives, SealedHeader};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::watch;

/// Tracks the chain info: canonical head, safe block, finalized block.
#[derive(Debug, Clone)]
pub struct ChainInfoTracker<N: NodePrimitives> {
    inner: Arc<ChainInfoInner<N>>,
}

impl<N> ChainInfoTracker<N>
where
    N: NodePrimitives,
    N::BlockHeader: BlockHeader,
{
    /// Create a new chain info container for the given canonical head and finalized header if it
    /// exists.
    pub fn new(
        head: SealedHeader<N::BlockHeader>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let (finalized_block, _) = watch::channel(finalized);
        let (safe_block, _) = watch::channel(safe);

        Self {
            inner: Arc::new(ChainInfoInner {
                last_forkchoice_update: RwLock::new(None),
                last_transition_configuration_exchange: RwLock::new(None),
                canonical_head_number: AtomicU64::new(head.number()),
                canonical_head: RwLock::new(head),
                safe_block,
                finalized_block,
            }),
        }
    }

    /// Returns the [`ChainInfo`] for the canonical head.
    pub fn chain_info(&self) -> ChainInfo {
        let inner = self.inner.canonical_head.read();
        ChainInfo { best_hash: inner.hash(), best_number: inner.number() }
    }

    /// Update the timestamp when we received a forkchoice update.
    pub fn on_forkchoice_update_received(&self) {
        self.inner.last_forkchoice_update.write().replace(Instant::now());
    }

    /// Returns the instant when we received the latest forkchoice update.
    pub fn last_forkchoice_update_received_at(&self) -> Option<Instant> {
        *self.inner.last_forkchoice_update.read()
    }

    /// Update the timestamp when we exchanged a transition configuration.
    pub fn on_transition_configuration_exchanged(&self) {
        self.inner.last_transition_configuration_exchange.write().replace(Instant::now());
    }

    /// Returns the instant when we exchanged the transition configuration last time.
    pub fn last_transition_configuration_exchanged_at(&self) -> Option<Instant> {
        *self.inner.last_transition_configuration_exchange.read()
    }

    /// Returns the canonical head of the chain.
    pub fn get_canonical_head(&self) -> SealedHeader<N::BlockHeader> {
        self.inner.canonical_head.read().clone()
    }

    /// Returns the safe header of the chain.
    pub fn get_safe_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.safe_block.borrow().clone()
    }

    /// Returns the finalized header of the chain.
    pub fn get_finalized_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.finalized_block.borrow().clone()
    }

    /// Returns the canonical head of the chain.
    #[allow(dead_code)]
    pub fn get_canonical_num_hash(&self) -> BlockNumHash {
        self.inner.canonical_head.read().num_hash()
    }

    /// Returns the canonical head of the chain.
    pub fn get_canonical_block_number(&self) -> BlockNumber {
        self.inner.canonical_head_number.load(Ordering::Relaxed)
    }

    /// Returns the safe header of the chain.
    pub fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.safe_block.borrow().as_ref().map(SealedHeader::num_hash)
    }

    /// Returns the finalized header of the chain.
    pub fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.finalized_block.borrow().as_ref().map(SealedHeader::num_hash)
    }

    /// Sets the canonical head of the chain.
    pub fn set_canonical_head(&self, header: SealedHeader<N::BlockHeader>) {
        let number = header.number();
        *self.inner.canonical_head.write() = header;

        // also update the atomic number.
        self.inner.canonical_head_number.store(number, Ordering::Relaxed);
    }

    /// Sets the safe header of the chain.
    pub fn set_safe(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.safe_block.send_if_modified(|current_header| {
            if current_header.as_ref().map(SealedHeader::hash) != Some(header.hash()) {
                let _ = current_header.replace(header);
                return true
            }

            false
        });
    }

    /// Sets the finalized header of the chain.
    pub fn set_finalized(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.finalized_block.send_if_modified(|current_header| {
            if current_header.as_ref().map(SealedHeader::hash) != Some(header.hash()) {
                let _ = current_header.replace(header);
                return true
            }

            false
        });
    }

    /// Subscribe to the finalized block.
    pub fn subscribe_finalized_block(
        &self,
    ) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.finalized_block.subscribe()
    }

    /// Subscribe to the safe block.
    pub fn subscribe_safe_block(&self) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.safe_block.subscribe()
    }
}

/// Container type for all chain info fields
#[derive(Debug)]
struct ChainInfoInner<N: NodePrimitives = reth_primitives::EthPrimitives> {
    /// Timestamp when we received the last fork choice update.
    ///
    /// This is mainly used to track if we're connected to a beacon node.
    last_forkchoice_update: RwLock<Option<Instant>>,
    /// Timestamp when we exchanged the transition configuration last time.
    ///
    /// This is mainly used to track if we're connected to a beacon node.
    last_transition_configuration_exchange: RwLock<Option<Instant>>,
    /// Tracks the number of the `canonical_head`.
    canonical_head_number: AtomicU64,
    /// The canonical head of the chain.
    canonical_head: RwLock<SealedHeader<N::BlockHeader>>,
    /// The block that the beacon node considers safe.
    safe_block: watch::Sender<Option<SealedHeader<N::BlockHeader>>>,
    /// The block that the beacon node considers finalized.
    finalized_block: watch::Sender<Option<SealedHeader<N::BlockHeader>>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::EthPrimitives;
    use reth_testing_utils::{generators, generators::random_header};

    #[test]
    fn test_chain_info() {
        // Create a random header
        let mut rng = generators::rng();
        let header = random_header(&mut rng, 10, None);

        // Create a new chain info tracker with the header
        let tracker: ChainInfoTracker<EthPrimitives> =
            ChainInfoTracker::new(header.clone(), None, None);

        // Fetch the chain information from the tracker
        let chain_info = tracker.chain_info();

        // Verify that the chain information matches the header
        assert_eq!(chain_info.best_number, header.number);
        assert_eq!(chain_info.best_hash, header.hash());
    }

    #[test]
    fn test_on_forkchoice_update_received() {
        // Create a random block header
        let mut rng = generators::rng();
        let header = random_header(&mut rng, 10, None);

        // Create a new chain info tracker with the header
        let tracker: ChainInfoTracker<EthPrimitives> = ChainInfoTracker::new(header, None, None);

        // Assert that there has been no forkchoice update yet (the timestamp is None)
        assert!(tracker.last_forkchoice_update_received_at().is_none());

        // Call the method to record the receipt of a forkchoice update
        tracker.on_forkchoice_update_received();

        // Assert that there is now a timestamp indicating when the forkchoice update was received
        assert!(tracker.last_forkchoice_update_received_at().is_some());
    }

    #[test]
    fn test_on_transition_configuration_exchanged() {
        // Create a random header
        let mut rng = generators::rng();
        let header = random_header(&mut rng, 10, None);

        // Create a new chain info tracker with the header
        let tracker: ChainInfoTracker<EthPrimitives> = ChainInfoTracker::new(header, None, None);

        // Assert that there has been no transition configuration exchange yet (the timestamp is
        // None)
        assert!(tracker.last_transition_configuration_exchanged_at().is_none());

        // Call the method to record the transition configuration exchange
        tracker.on_transition_configuration_exchanged();

        // Assert that there is now a timestamp indicating when the transition configuration
        // exchange occurred
        assert!(tracker.last_transition_configuration_exchanged_at().is_some());
    }

    #[test]
    fn test_set_canonical_head() {
        // Create a random number generator
        let mut rng = generators::rng();
        // Generate two random headers for testing
        let header1 = random_header(&mut rng, 10, None);
        let header2 = random_header(&mut rng, 20, None);

        // Create a new chain info tracker with the first header
        let tracker: ChainInfoTracker<EthPrimitives> = ChainInfoTracker::new(header1, None, None);

        // Set the second header as the canonical head of the tracker
        tracker.set_canonical_head(header2.clone());

        // Assert that the tracker now uses the second header as its canonical head
        let canonical_head = tracker.get_canonical_head();
        assert_eq!(canonical_head, header2);
    }

    #[test]
    fn test_set_safe() {
        // Create a random number generator
        let mut rng = generators::rng();

        // Case 1: basic test
        // Generate two random headers for the test
        let header1 = random_header(&mut rng, 10, None);
        let header2 = random_header(&mut rng, 20, None);

        // Create a new chain info tracker with the first header (header1)
        let tracker: ChainInfoTracker<EthPrimitives> = ChainInfoTracker::new(header1, None, None);

        // Call the set_safe method with the second header (header2)
        tracker.set_safe(header2.clone());

        // Verify that the tracker now has header2 as the safe block
        let safe_header = tracker.get_safe_header();
        assert!(safe_header.is_some()); // Ensure a safe header is present
        let safe_header = safe_header.unwrap();
        assert_eq!(safe_header, header2);

        // Case 2: call with the same header as the current safe block
        // Call set_safe again with the same header (header2)
        tracker.set_safe(header2.clone());

        // Verify that nothing changes and the safe header remains the same
        let same_safe_header = tracker.get_safe_header();
        assert!(same_safe_header.is_some());
        let same_safe_header = same_safe_header.unwrap();
        assert_eq!(same_safe_header, header2);

        // Case 3: call with a different (new) header
        // Generate a third header with a higher block number
        let header3 = random_header(&mut rng, 30, None);

        // Call set_safe with this new header (header3)
        tracker.set_safe(header3.clone());

        // Verify that the safe header is updated with the new header
        let updated_safe_header = tracker.get_safe_header();
        assert!(updated_safe_header.is_some());
        let updated_safe_header = updated_safe_header.unwrap();
        assert_eq!(updated_safe_header, header3);
    }

    #[test]
    fn test_set_finalized() {
        // Create a random number generator
        let mut rng = generators::rng();

        // Generate random headers for testing
        let header1 = random_header(&mut rng, 10, None);
        let header2 = random_header(&mut rng, 20, None);
        let header3 = random_header(&mut rng, 30, None);

        // Create a new chain info tracker with the first header
        let tracker: ChainInfoTracker<EthPrimitives> = ChainInfoTracker::new(header1, None, None);

        // Initial state: finalize header should be None
        assert!(tracker.get_finalized_header().is_none());

        // Set the second header as the finalized header
        tracker.set_finalized(header2.clone());

        // Assert that the tracker now uses the second header as its finalized block
        let finalized_header = tracker.get_finalized_header();
        assert!(finalized_header.is_some());
        let finalized_header = finalized_header.unwrap();
        assert_eq!(finalized_header, header2);

        // Case 2: attempt to set the same finalized header again
        tracker.set_finalized(header2.clone());

        // The finalized header should remain unchanged
        let unchanged_finalized_header = tracker.get_finalized_header();
        assert_eq!(unchanged_finalized_header.unwrap(), header2); // Should still be header2

        // Case 3: set a higher block number as finalized
        tracker.set_finalized(header3.clone());

        // The finalized header should now be updated to header3
        let updated_finalized_header = tracker.get_finalized_header();
        assert!(updated_finalized_header.is_some());
        assert_eq!(updated_finalized_header.unwrap(), header3);
    }

    #[test]
    fn test_get_finalized_num_hash() {
        // Create a random header
        let mut rng = generators::rng();
        let finalized_header = random_header(&mut rng, 10, None);

        // Create a new chain info tracker with the finalized header
        let tracker: ChainInfoTracker<EthPrimitives> =
            ChainInfoTracker::new(finalized_header.clone(), Some(finalized_header.clone()), None);

        // Assert that the BlockNumHash returned matches the finalized header
        assert_eq!(tracker.get_finalized_num_hash(), Some(finalized_header.num_hash()));
    }

    #[test]
    fn test_get_safe_num_hash() {
        // Create a random header
        let mut rng = generators::rng();
        let safe_header = random_header(&mut rng, 10, None);

        // Create a new chain info tracker with the safe header
        let tracker: ChainInfoTracker<EthPrimitives> =
            ChainInfoTracker::new(safe_header.clone(), None, None);
        tracker.set_safe(safe_header.clone());

        // Assert that the BlockNumHash returned matches the safe header
        assert_eq!(tracker.get_safe_num_hash(), Some(safe_header.num_hash()));
    }
}
