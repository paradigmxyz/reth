#![allow(unused)]
//! Test helper impls for generating bodies
use reth_primitives::SealedHeader;

/// Returns a new [SealedHeader] that's the child header of the given `parent`.
pub(crate) fn child_header(parent: &SealedHeader) -> SealedHeader {
    let mut child = parent.as_ref().clone();
    child.number += 1;
    child.parent_hash = parent.hash_slow();
    let hash = child.hash_slow();
    SealedHeader::new(child, hash)
}
