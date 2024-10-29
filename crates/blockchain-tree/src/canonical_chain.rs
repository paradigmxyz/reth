use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber};
use std::collections::BTreeMap;

/// This keeps track of (non-finalized) blocks of the canonical chain.
///
/// This is a wrapper type around an ordered set of block numbers and hashes that belong to the
/// canonical chain that is not yet finalized.
#[derive(Debug, Clone, Default)]
pub(crate) struct CanonicalChain {
    /// All blocks of the canonical chain in order of their block number.
    chain: BTreeMap<BlockNumber, BlockHash>,
}

impl CanonicalChain {
    pub(crate) const fn new(chain: BTreeMap<BlockNumber, BlockHash>) -> Self {
        Self { chain }
    }

    /// Replaces the current chain with the given one.
    #[inline]
    pub(crate) fn replace(&mut self, chain: BTreeMap<BlockNumber, BlockHash>) {
        self.chain = chain;
    }

    /// Returns the block hash of the (non-finalized) canonical block with the given number.
    #[inline]
    pub(crate) fn canonical_hash(&self, number: &BlockNumber) -> Option<BlockHash> {
        self.chain.get(number).copied()
    }

    /// Returns the block number of the (non-finalized) canonical block with the given hash.
    #[inline]
    pub(crate) fn canonical_number(&self, block_hash: &BlockHash) -> Option<BlockNumber> {
        self.chain.iter().find_map(|(number, hash)| (hash == block_hash).then_some(*number))
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
    pub(crate) const fn inner(&self) -> &BTreeMap<BlockNumber, BlockHash> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_canonical_chain() {
        // Initialize a chain with some blocks
        let mut initial_chain = BTreeMap::new();
        initial_chain.insert(BlockNumber::from(1u64), BlockHash::from([0x01; 32]));
        initial_chain.insert(BlockNumber::from(2u64), BlockHash::from([0x02; 32]));

        let mut canonical_chain = CanonicalChain::new(initial_chain.clone());

        // Verify initial chain state
        assert_eq!(canonical_chain.chain.len(), 2);
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(1u64)),
            Some(&BlockHash::from([0x01; 32]))
        );

        // Replace with a new chain
        let mut new_chain = BTreeMap::new();
        new_chain.insert(BlockNumber::from(3u64), BlockHash::from([0x03; 32]));
        new_chain.insert(BlockNumber::from(4u64), BlockHash::from([0x04; 32]));
        new_chain.insert(BlockNumber::from(5u64), BlockHash::from([0x05; 32]));

        canonical_chain.replace(new_chain.clone());

        // Verify replaced chain state
        assert_eq!(canonical_chain.chain.len(), 3);
        assert!(!canonical_chain.chain.contains_key(&BlockNumber::from(1u64)));
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(3u64)),
            Some(&BlockHash::from([0x03; 32]))
        );
    }

    #[test]
    fn test_canonical_hash_canonical_chain() {
        // Initialize a chain with some blocks
        let mut chain = BTreeMap::new();
        chain.insert(BlockNumber::from(1u64), BlockHash::from([0x01; 32]));
        chain.insert(BlockNumber::from(2u64), BlockHash::from([0x02; 32]));
        chain.insert(BlockNumber::from(3u64), BlockHash::from([0x03; 32]));

        // Create an instance of a canonical chain
        let canonical_chain = CanonicalChain::new(chain.clone());

        // Check that the function returns the correct hash for a given block number
        let block_number = BlockNumber::from(2u64);
        let expected_hash = BlockHash::from([0x02; 32]);
        assert_eq!(canonical_chain.canonical_hash(&block_number), Some(expected_hash));

        // Check that a non-existent block returns None
        let non_existent_block = BlockNumber::from(5u64);
        assert_eq!(canonical_chain.canonical_hash(&non_existent_block), None);
    }

    #[test]
    fn test_canonical_number_canonical_chain() {
        // Initialize a chain with some blocks
        let mut chain = BTreeMap::new();
        chain.insert(BlockNumber::from(1u64), BlockHash::from([0x01; 32]));
        chain.insert(BlockNumber::from(2u64), BlockHash::from([0x02; 32]));
        chain.insert(BlockNumber::from(3u64), BlockHash::from([0x03; 32]));

        // Create an instance of a canonical chain
        let canonical_chain = CanonicalChain::new(chain.clone());

        // Check that the function returns the correct block number for a given block hash
        let block_hash = BlockHash::from([0x02; 32]);
        let expected_number = BlockNumber::from(2u64);
        assert_eq!(canonical_chain.canonical_number(&block_hash), Some(expected_number));

        // Check that a non-existent block hash returns None
        let non_existent_hash = BlockHash::from([0x05; 32]);
        assert_eq!(canonical_chain.canonical_number(&non_existent_hash), None);
    }

    #[test]
    fn test_extend_canonical_chain() {
        // Initialize an empty chain
        let mut canonical_chain = CanonicalChain::new(BTreeMap::new());

        // Create an iterator with some blocks
        let blocks = vec![
            (BlockNumber::from(1u64), BlockHash::from([0x01; 32])),
            (BlockNumber::from(2u64), BlockHash::from([0x02; 32])),
        ]
        .into_iter();

        // Extend the chain with the created blocks
        canonical_chain.extend(blocks);

        // Check if the blocks were added correctly
        assert_eq!(canonical_chain.chain.len(), 2);
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(1u64)),
            Some(&BlockHash::from([0x01; 32]))
        );
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(2u64)),
            Some(&BlockHash::from([0x02; 32]))
        );

        // Test extending with additional blocks again
        let more_blocks = vec![(BlockNumber::from(3u64), BlockHash::from([0x03; 32]))].into_iter();
        canonical_chain.extend(more_blocks);

        assert_eq!(canonical_chain.chain.len(), 3);
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(3u64)),
            Some(&BlockHash::from([0x03; 32]))
        );
    }

    #[test]
    fn test_retain_canonical_chain() {
        // Initialize a chain with some blocks
        let mut chain = BTreeMap::new();
        chain.insert(BlockNumber::from(1u64), BlockHash::from([0x01; 32]));
        chain.insert(BlockNumber::from(2u64), BlockHash::from([0x02; 32]));
        chain.insert(BlockNumber::from(3u64), BlockHash::from([0x03; 32]));

        // Create an instance of CanonicalChain
        let mut canonical_chain = CanonicalChain::new(chain);

        // Retain only blocks with even block numbers
        canonical_chain.retain(|number, _| number % 2 == 0);

        // Check if the chain only contains the block with number 2
        assert_eq!(canonical_chain.chain.len(), 1);
        assert_eq!(
            canonical_chain.chain.get(&BlockNumber::from(2u64)),
            Some(&BlockHash::from([0x02; 32]))
        );

        // Ensure that the blocks with odd numbers were removed
        assert_eq!(canonical_chain.chain.get(&BlockNumber::from(1u64)), None);
        assert_eq!(canonical_chain.chain.get(&BlockNumber::from(3u64)), None);
    }

    #[test]
    fn test_tip_canonical_chain() {
        // Initialize a chain with some blocks
        let mut chain = BTreeMap::new();
        chain.insert(BlockNumber::from(1u64), BlockHash::from([0x01; 32]));
        chain.insert(BlockNumber::from(2u64), BlockHash::from([0x02; 32]));
        chain.insert(BlockNumber::from(3u64), BlockHash::from([0x03; 32]));

        // Create an instance of a canonical chain
        let canonical_chain = CanonicalChain::new(chain);

        // Call the tip method and verify the returned value
        let tip = canonical_chain.tip();
        assert_eq!(tip.number, BlockNumber::from(3u64));
        assert_eq!(tip.hash, BlockHash::from([0x03; 32]));

        // Test with an empty chain
        let empty_chain = CanonicalChain::new(BTreeMap::new());
        let empty_tip = empty_chain.tip();
        assert_eq!(empty_tip.number, BlockNumber::default());
        assert_eq!(empty_tip.hash, BlockHash::default());
    }
}
