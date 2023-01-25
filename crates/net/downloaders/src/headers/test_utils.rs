use once_cell::sync::Lazy;
use reth_interfaces::{consensus::Consensus, test_utils::TestConsensus};
use reth_primitives::SealedHeader;
use std::sync::Arc;

pub(crate) static CONSENSUS: Lazy<Arc<dyn Consensus>> =
    Lazy::new(|| Arc::new(TestConsensus::default()));

/// Returns a new [SealedHeader] that's the child header of the given `parent`.
pub(crate) fn child_header(parent: &SealedHeader) -> SealedHeader {
    let mut child = parent.as_ref().clone();
    child.number += 1;
    child.parent_hash = parent.hash_slow();
    let hash = child.hash_slow();
    SealedHeader::new(child, hash)
}
