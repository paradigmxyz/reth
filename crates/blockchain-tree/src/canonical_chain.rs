use reth_primitives::{BlockHash, BlockNumHash, BlockNumber};
use std::collections::BTreeMap;

/// This keeps track of all blocks of the canonical chain.
///
/// This is a wrapper type around an ordered set of block numbers and hashes that belong to the
/// canonical chain.
#[derive(Debug, Clone, Default)]
pub(crate) struct CanonicalChain {
    /// All blocks of the canonical chain in order.
    chain: BTreeMap<BlockNumber, BlockHash>,
}

impl CanonicalChain {
    pub(crate) fn new(chain: BTreeMap<BlockNumber, BlockHash>) -> Self {
        Self { chain }
    }

    /// Replaces the current chain with the given one.
    #[inline]
    pub(crate) fn replace(&mut self, chain: BTreeMap<BlockNumber, BlockHash>) {
        self.chain = chain;
    }

    /// Returns the block hash of the canonical block with the given number.
    #[inline]
    pub(crate) fn canonical_hash(&self, number: &BlockNumber) -> Option<BlockHash> {
        self.chain.get(number).cloned()
    }

    /// Returns the block number of the canonical block with the given hash.
    #[inline]
    pub(crate) fn canonical_number(&self, block_hash: BlockHash) -> Option<BlockNumber> {
        self.chain.iter().find_map(
            |(number, hash)| {
                if *hash == block_hash {
                    Some(*number)
                } else {
                    None
                }
            },
        )
    }

    /// Returns the block number of the canonical block with the given hash.
    ///
    /// Returns `None` if no block could be found in the canonical chain.
    #[inline]
    pub(crate) fn get_canonical_block_number(
        &self,
        last_finalized_block: BlockNumber,
        block_hash: &BlockHash,
    ) -> Option<BlockNumber> {
        self.chain
            .range(last_finalized_block..)
            .find_map(|(num, &h)| (h == *block_hash).then_some(*num))
    }

    /// Extends all items from the given iterator to the chain.
    #[inline]
    pub(crate) fn extend(&mut self, blocks: impl Iterator<Item = (BlockNumber, BlockHash)>) {
        self.chain.extend(blocks)
    }

    /// Retains only the elements specified by the predicate.
    #[inline]
    pub(crate) fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&BlockNumber, &mut BlockHash) -> bool,
    {
        self.chain.retain(f)
    }

    #[inline]
    pub(crate) fn inner(&self) -> &BTreeMap<BlockNumber, BlockHash> {
        &self.chain
    }

    #[inline]
    pub(crate) fn tip(&self) -> BlockNumHash {
        self.chain
            .last_key_value()
            .map(|(&number, &hash)| BlockNumHash { number, hash })
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (BlockNumber, BlockHash)> + '_ {
        self.chain.iter().map(|(&number, &hash)| (number, hash))
    }

    #[inline]
    pub(crate) fn into_iter(self) -> impl Iterator<Item = (BlockNumber, BlockHash)> {
        self.chain.into_iter()
    }
}
