use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_primitives::{BlockNumHash, BlockNumber, SealedHeader};
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
pub struct ChainInfoTracker {
    inner: Arc<ChainInfoInner>,
}

impl ChainInfoTracker {
    /// Create a new chain info container for the given canonical head.
    pub fn new(head: SealedHeader, finalized: Option<SealedHeader>) -> Self {
        let (finalized_block, _) = watch::channel(finalized.clone());
        let (safe_block, _) = watch::channel(finalized);

        Self {
            inner: Arc::new(ChainInfoInner {
                last_forkchoice_update: RwLock::new(None),
                last_transition_configuration_exchange: RwLock::new(None),
                canonical_head_number: AtomicU64::new(head.number),
                canonical_head: RwLock::new(head),
                safe_block,
                finalized_block,
            }),
        }
    }

    /// Returns the [`ChainInfo`] for the canonical head.
    pub fn chain_info(&self) -> ChainInfo {
        let inner = self.inner.canonical_head.read();
        ChainInfo { best_hash: inner.hash(), best_number: inner.number }
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
    pub fn get_canonical_head(&self) -> SealedHeader {
        self.inner.canonical_head.read().clone()
    }

    /// Returns the safe header of the chain.
    pub fn get_safe_header(&self) -> Option<SealedHeader> {
        self.inner.safe_block.borrow().clone()
    }

    /// Returns the finalized header of the chain.
    pub fn get_finalized_header(&self) -> Option<SealedHeader> {
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
    #[allow(dead_code)]
    pub fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        let h = self.inner.safe_block.borrow();
        h.as_ref().map(|h| h.num_hash())
    }

    /// Returns the finalized header of the chain.
    #[allow(dead_code)]
    pub fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        let h = self.inner.finalized_block.borrow();
        h.as_ref().map(|h| h.num_hash())
    }

    /// Sets the canonical head of the chain.
    pub fn set_canonical_head(&self, header: SealedHeader) {
        let number = header.number;
        *self.inner.canonical_head.write() = header;

        // also update the atomic number.
        self.inner.canonical_head_number.store(number, Ordering::Relaxed);
    }

    /// Sets the safe header of the chain.
    pub fn set_safe(&self, header: SealedHeader) {
        self.inner.safe_block.send_modify(|h| {
            let _ = h.replace(header);
        });
    }

    /// Sets the finalized header of the chain.
    pub fn set_finalized(&self, header: SealedHeader) {
        self.inner.finalized_block.send_modify(|h| {
            let _ = h.replace(header);
        });
    }
}

/// Container type for all chain info fields
#[derive(Debug)]
struct ChainInfoInner {
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
    canonical_head: RwLock<SealedHeader>,
    /// The block that the beacon node considers safe.
    safe_block: watch::Sender<Option<SealedHeader>>,
    /// The block that the beacon node considers finalized.
    finalized_block: watch::Sender<Option<SealedHeader>>,
}
