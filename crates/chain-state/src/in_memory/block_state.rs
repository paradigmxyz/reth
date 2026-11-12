use super::ExecutedBlock;
use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_eips::{BlockHashOrNumber, BlockNumHash};
use alloy_primitives::{TxHash, B256};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{BlockBody as _, IndexedTx, NodePrimitives};
use std::sync::Arc;

/// An executed block together with an immutable snapshot of its linked in-memory ancestry.
///
/// [`Self::chain`] yields this block and the ancestors it was linked to when the snapshot was
/// created. [`Self::anchor`] is the parent of the oldest of them: the block the chain builds on,
/// which is not part of the snapshot.
///
/// A snapshot never changes: blocks that are removed from, or added to, an
/// [`InMemoryState`](super::InMemoryState) later do not show up in it. Two snapshots of the same
/// block can therefore have different chains. Compare blocks by hash.
#[derive(Debug, Clone)]
pub struct BlockState<N: NodePrimitives = EthPrimitives> {
    /// The executed block that determines the state after this block has been executed.
    block: ExecutedBlock<N>,
    /// The snapshot of the parent block this block is linked to, if there is one.
    parent: Option<Arc<Self>>,
}

impl<N: NodePrimitives> PartialEq for BlockState<N> {
    fn eq(&self, other: &Self) -> bool {
        self.block == other.block && self.parent == other.parent
    }
}

impl<N: NodePrimitives> BlockState<N> {
    /// Creates the state of `block` without linked ancestors.
    pub const fn new(block: ExecutedBlock<N>) -> Self {
        Self { block, parent: None }
    }

    /// Creates the state of `block` linked to `parent`, the state of its parent block.
    pub const fn with_parent(block: ExecutedBlock<N>, parent: Option<Arc<Self>>) -> Self {
        Self { block, parent }
    }

    /// Returns the number and hash of the block this chain builds on: the parent of the oldest
    /// block of [`Self::chain`].
    ///
    /// It is persisted, or it is an in-memory block the chain was not linked to.
    pub fn anchor(&self) -> BlockNumHash {
        let mut current = self;
        while let Some(parent) = &current.parent {
            current = parent;
        }
        current.block.recovered_block().parent_num_hash()
    }

    /// Returns the hash of the parent block.
    pub fn parent_hash(&self) -> B256 {
        self.block.recovered_block().parent_hash()
    }

    /// Returns the executed block that determines the state.
    pub fn block(&self) -> ExecutedBlock<N> {
        self.block.clone()
    }

    /// Returns a reference to the executed block that determines the state.
    pub const fn block_ref(&self) -> &ExecutedBlock<N> {
        &self.block
    }

    /// Returns the hash of executed block that determines the state.
    pub fn hash(&self) -> B256 {
        self.block.recovered_block().hash()
    }

    /// Returns the block number of executed block that determines the state.
    pub fn number(&self) -> u64 {
        self.block.recovered_block().number()
    }

    /// Returns the state root after applying the executed block that determines
    /// the state.
    pub fn state_root(&self) -> B256 {
        self.block.recovered_block().state_root()
    }

    /// Returns the `Receipts` of executed block that determines the state.
    pub fn receipts(&self) -> &Vec<N::Receipt> {
        &self.block.execution_outcome().receipts
    }

    /// Returns a vector of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    ///
    /// This clones the vector of receipts. To avoid it, use [`Self::executed_block_receipts_ref`].
    pub fn executed_block_receipts(&self) -> Vec<N::Receipt> {
        self.receipts().clone()
    }

    /// Returns a slice of `Receipt` of executed block that determines the state.
    /// We assume that the `Receipts` in the executed block `ExecutionOutcome`
    /// has only one element corresponding to the executed block associated to
    /// the state.
    pub fn executed_block_receipts_ref(&self) -> &[N::Receipt] {
        self.receipts()
    }

    /// Returns an iterator over the linked ancestors of this block.
    ///
    /// The block state order is newest to oldest (highest to lowest):
    /// `[5,4,3,2,1]`
    ///
    /// Note: This does not include self.
    pub fn parent_state_chain(&self) -> impl Iterator<Item = &Self> + '_ {
        std::iter::successors(self.parent.as_deref(), |state| state.parent.as_deref())
    }

    /// Returns an iterator over this block and its linked ancestors.
    ///
    /// The block state order is newest to oldest (highest to lowest), with self as the first
    /// element.
    pub fn chain(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |state| state.parent.as_deref())
    }

    /// Appends the parent chain of this [`BlockState`] to the given vector.
    ///
    /// Parents are appended in order from newest to oldest (highest to lowest).
    /// This does not include self, only the parent states.
    ///
    /// This is a convenience method equivalent to `chain.extend(self.parent_state_chain())`.
    pub fn append_parent_chain<'a>(&'a self, chain: &mut Vec<&'a Self>) {
        chain.extend(self.parent_state_chain());
    }

    /// Returns an owning iterator over this block and its linked ancestors, like [`Self::chain`].
    ///
    /// This yields the blocks from newest to oldest (highest to lowest).
    pub fn iter(self: Arc<Self>) -> impl Iterator<Item = Arc<Self>> {
        std::iter::successors(Some(self), |state| state.parent.clone())
    }

    /// Returns the hashes of [`Self::chain`] from newest to oldest, followed by the hash of the
    /// block the chain builds on, see [`Self::anchor`].
    pub fn chain_hashes(&self) -> impl Iterator<Item = B256> + '_ {
        std::iter::once(self.hash()).chain(self.chain().map(Self::parent_hash))
    }

    /// Tries to find a block by [`BlockHashOrNumber`] in the chain ending at this block.
    pub fn block_on_chain(&self, hash_or_num: BlockHashOrNumber) -> Option<&Self> {
        self.chain().find(|block| match hash_or_num {
            BlockHashOrNumber::Hash(hash) => block.hash() == hash,
            BlockHashOrNumber::Number(number) => block.number() == number,
        })
    }

    /// Tries to find a transaction by [`TxHash`] in the chain ending at this block.
    pub fn transaction_on_chain(&self, hash: TxHash) -> Option<N::SignedTx> {
        self.chain().find_map(|block_state| {
            block_state.block_ref().recovered_block().body().transaction_by_hash(&hash).cloned()
        })
    }

    /// Tries to find a transaction with meta by [`TxHash`] in the chain ending at this block.
    pub fn transaction_meta_on_chain(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        self.chain().find_map(|block_state| {
            block_state.find_indexed(tx_hash).map(|indexed| (indexed.tx().clone(), indexed.meta()))
        })
    }

    /// Finds a transaction by hash and returns it with its index and block context.
    pub fn find_indexed(&self, tx_hash: TxHash) -> Option<IndexedTx<'_, N::Block>> {
        self.block_ref().recovered_block().find_indexed(tx_hash)
    }

    /// Returns the snapshot of the parent block this block is linked to, if there is one.
    pub(super) const fn parent(&self) -> Option<&Arc<Self>> {
        self.parent.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use rand::Rng;
    use reth_ethereum_primitives::{EthPrimitives, Receipt};

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    fn create_mock_state_chain(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        num_blocks: u64,
    ) -> Vec<BlockState> {
        let mut chain = Vec::with_capacity(num_blocks as usize);
        let mut parent_hash = B256::random();
        let mut parent_state: Option<BlockState> = None;

        for i in 1..=num_blocks {
            let mut state = create_mock_state(test_block_builder, i, parent_hash);
            if let Some(parent) = parent_state {
                state.parent = Some(Arc::new(parent));
            }
            parent_hash = state.hash();
            parent_state = Some(state.clone());
            chain.push(state);
        }

        chain
    }

    #[test]
    fn test_state() {
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block = test_block_builder.get_executed_block_with_number(number, B256::random());

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
        assert_eq!(state.hash(), block.recovered_block().hash());
        assert_eq!(state.number(), number);
        assert_eq!(state.state_root(), block.recovered_block().state_root);
    }

    #[test]
    fn test_state_receipts() {
        let receipts = vec![vec![Receipt::default()]];
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block =
            test_block_builder.get_executed_block_with_receipts(receipts.clone(), B256::random());

        let state = BlockState::new(block);

        assert_eq!(state.receipts(), receipts.first().unwrap());
    }

    #[test]
    fn test_block_state_parent_blocks() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 4);

        let parents: Vec<_> = chain[3].parent_state_chain().collect();
        assert_eq!(parents.len(), 3);
        assert_eq!(parents[0].block().recovered_block().number, 3);
        assert_eq!(parents[1].block().recovered_block().number, 2);
        assert_eq!(parents[2].block().recovered_block().number, 1);

        let parents: Vec<_> = chain[2].parent_state_chain().collect();
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].block().recovered_block().number, 2);
        assert_eq!(parents[1].block().recovered_block().number, 1);

        assert_eq!(chain[0].parent_state_chain().count(), 0);
    }

    #[test]
    fn test_block_state_single_block_state_chain() {
        let single_block_number = 1;
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let single_block =
            create_mock_state(&mut test_block_builder, single_block_number, B256::random());
        let single_block_hash = single_block.block().recovered_block().hash();

        assert_eq!(single_block.parent_state_chain().count(), 0);

        let block_state_chain = single_block.chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, single_block_number);
        assert_eq!(block_state_chain[0].block().recovered_block().hash(), single_block_hash);
    }

    #[test]
    fn test_block_state_chain() {
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let chain = create_mock_state_chain(&mut test_block_builder, 3);

        let block_state_chain = chain[2].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 3);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 3);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[2].block().recovered_block().number, 1);

        let block_state_chain = chain[1].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 2);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 2);
        assert_eq!(block_state_chain[1].block().recovered_block().number, 1);

        let block_state_chain = chain[0].chain().collect::<Vec<_>>();
        assert_eq!(block_state_chain.len(), 1);
        assert_eq!(block_state_chain[0].block().recovered_block().number, 1);
    }
}
