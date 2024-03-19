//! Contains [Chain], a chain of blocks and their final state.

use crate::bundle_state::BundleStateWithReceipts;
use reth_interfaces::{executor::BlockExecutionError, RethResult};
use reth_primitives::{
    Address, BlockHash, BlockNumHash, BlockNumber, ForkBlock, Receipt, SealedBlock,
    SealedBlockWithSenders, SealedHeader, TransactionSigned, TransactionSignedEcRecovered, TxHash,
};
use reth_trie::updates::TrieUpdates;
use revm::db::BundleState;
use std::{borrow::Cow, collections::BTreeMap, fmt};

/// A chain of blocks and their final state.
///
/// The chain contains the state of accounts after execution of its blocks,
/// changesets for those blocks (and their transactions), as well as the blocks themselves.
///
/// Used inside the BlockchainTree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// All blocks in this chain.
    blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
    /// The state of all accounts after execution of the _all_ blocks in this chain's range from
    /// [Chain::first] to [Chain::tip], inclusive.
    ///
    /// This state also contains the individual changes that lead to the current state.
    state: BundleStateWithReceipts,
    /// State trie updates after block is added to the chain.
    /// NOTE: Currently, trie updates are present only if the block extends canonical chain.
    trie_updates: Option<TrieUpdates>,
}

impl Chain {
    /// Create new Chain from blocks and state.
    pub fn new(
        blocks: impl IntoIterator<Item = SealedBlockWithSenders>,
        state: BundleStateWithReceipts,
        trie_updates: Option<TrieUpdates>,
    ) -> Self {
        Self {
            blocks: BTreeMap::from_iter(blocks.into_iter().map(|b| (b.number, b))),
            state,
            trie_updates,
        }
    }

    /// Create new Chain from a single block and its state.
    pub fn from_block(
        block: SealedBlockWithSenders,
        state: BundleStateWithReceipts,
        trie_updates: Option<TrieUpdates>,
    ) -> Self {
        Self::new([block], state, trie_updates)
    }

    /// Get the blocks in this chain.
    pub fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
        &self.blocks
    }

    /// Consumes the type and only returns the blocks in this chain.
    pub fn into_blocks(self) -> BTreeMap<BlockNumber, SealedBlockWithSenders> {
        self.blocks
    }

    /// Returns an iterator over all headers in the block with increasing block numbers.
    pub fn headers(&self) -> impl Iterator<Item = SealedHeader> + '_ {
        self.blocks.values().map(|block| block.header.clone())
    }

    /// Get cached trie updates for this chain.
    pub fn trie_updates(&self) -> Option<&TrieUpdates> {
        self.trie_updates.as_ref()
    }

    /// Get post state of this chain
    pub fn state(&self) -> &BundleStateWithReceipts {
        &self.state
    }

    /// Prepends the given state to the current state.
    pub fn prepend_state(&mut self, state: BundleState) {
        self.state.prepend_state(state);
        self.trie_updates.take(); // invalidate cached trie updates
    }

    /// Return true if chain is empty and has no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Return block number of the block hash.
    pub fn block_number(&self, block_hash: BlockHash) -> Option<BlockNumber> {
        self.blocks.iter().find_map(|(num, block)| (block.hash() == block_hash).then_some(*num))
    }

    /// Returns the block with matching hash.
    pub fn block(&self, block_hash: BlockHash) -> Option<&SealedBlock> {
        self.block_with_senders(block_hash).map(|block| &block.block)
    }

    /// Returns the block with matching hash.
    pub fn block_with_senders(&self, block_hash: BlockHash) -> Option<&SealedBlockWithSenders> {
        self.blocks.iter().find_map(|(_num, block)| (block.hash() == block_hash).then_some(block))
    }

    /// Return post state of the block at the `block_number` or None if block is not known
    pub fn state_at_block(&self, block_number: BlockNumber) -> Option<BundleStateWithReceipts> {
        if self.tip().number == block_number {
            return Some(self.state.clone())
        }

        if self.blocks.contains_key(&block_number) {
            let mut state = self.state.clone();
            state.revert_to(block_number);
            return Some(state)
        }
        None
    }

    /// Destructure the chain into its inner components, the blocks and the state at the tip of the
    /// chain.
    pub fn into_inner(
        self,
    ) -> (ChainBlocks<'static>, BundleStateWithReceipts, Option<TrieUpdates>) {
        (ChainBlocks { blocks: Cow::Owned(self.blocks) }, self.state, self.trie_updates)
    }

    /// Destructure the chain into its inner components, the blocks and the state at the tip of the
    /// chain.
    pub fn inner(&self) -> (ChainBlocks<'_>, &BundleStateWithReceipts) {
        (ChainBlocks { blocks: Cow::Borrowed(&self.blocks) }, &self.state)
    }

    /// Returns an iterator over all the receipts of the blocks in the chain.
    pub fn block_receipts_iter(&self) -> impl Iterator<Item = &Vec<Option<Receipt>>> + '_ {
        self.state.receipts().iter()
    }

    /// Returns an iterator over all blocks in the chain with increasing block number.
    pub fn blocks_iter(&self) -> impl Iterator<Item = &SealedBlockWithSenders> + '_ {
        self.blocks().iter().map(|block| block.1)
    }

    /// Returns an iterator over all blocks and their receipts in the chain.
    pub fn blocks_and_receipts(
        &self,
    ) -> impl Iterator<Item = (&SealedBlockWithSenders, &Vec<Option<Receipt>>)> + '_ {
        self.blocks_iter().zip(self.block_receipts_iter())
    }

    /// Get the block at which this chain forked.
    #[track_caller]
    pub fn fork_block(&self) -> ForkBlock {
        let first = self.first();
        ForkBlock { number: first.number.saturating_sub(1), hash: first.parent_hash }
    }

    /// Get the first block in this chain.
    #[track_caller]
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain has at least one block for first").1
    }

    /// Get the tip of the chain.
    ///
    /// # Note
    ///
    /// Chains always have at least one block.
    #[track_caller]
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain should have at least one block").1
    }

    /// Returns length of the chain.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Get all receipts for the given block.
    pub fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<&Receipt>> {
        let num = self.block_number(block_hash)?;
        self.state.receipts_by_block(num).iter().map(Option::as_ref).collect()
    }

    /// Get all receipts with attachment.
    ///
    /// Attachment includes block number, block hash, transaction hash and transaction index.
    pub fn receipts_with_attachment(&self) -> Vec<BlockReceipts> {
        let mut receipt_attach = Vec::new();
        for ((block_num, block), receipts) in self.blocks().iter().zip(self.state.receipts().iter())
        {
            let mut tx_receipts = Vec::new();
            for (tx, receipt) in block.body.iter().zip(receipts.iter()) {
                tx_receipts.push((
                    tx.hash(),
                    receipt.as_ref().expect("receipts have not been pruned").clone(),
                ));
            }
            let block_num_hash = BlockNumHash::new(*block_num, block.hash());
            receipt_attach.push(BlockReceipts { block: block_num_hash, tx_receipts });
        }
        receipt_attach
    }

    /// Append a single block with state to the chain.
    /// This method assumes that blocks attachment to the chain has already been validated.
    pub fn append_block(
        &mut self,
        block: SealedBlockWithSenders,
        state: BundleStateWithReceipts,
        trie_updates: Option<TrieUpdates>,
    ) {
        self.blocks.insert(block.number, block);
        self.state.extend(state);
        self.append_trie_updates(trie_updates);
    }

    /// Merge two chains by appending the given chain into the current one.
    ///
    /// The state of accounts for this chain is set to the state of the newest chain.
    pub fn append_chain(&mut self, other: Chain) -> RethResult<()> {
        let chain_tip = self.tip();
        let other_fork_block = other.fork_block();
        if chain_tip.hash() != other_fork_block.hash {
            return Err(BlockExecutionError::AppendChainDoesntConnect {
                chain_tip: Box::new(chain_tip.num_hash()),
                other_chain_fork: Box::new(other_fork_block),
            }
            .into())
        }

        // Insert blocks from other chain
        self.blocks.extend(other.blocks);
        self.state.extend(other.state);
        self.append_trie_updates(other.trie_updates);

        Ok(())
    }

    /// Append trie updates.
    /// If existing or incoming trie updates are not set, reset as neither is valid anymore.
    fn append_trie_updates(&mut self, other_trie_updates: Option<TrieUpdates>) {
        if let Some((trie_updates, other)) = self.trie_updates.as_mut().zip(other_trie_updates) {
            // Extend trie updates.
            trie_updates.extend(other);
        } else {
            // Reset trie updates as they are no longer valid.
            self.trie_updates.take();
        }
    }

    /// Split this chain at the given block.
    ///
    /// The given block will be the last block in the first returned chain.
    ///
    /// If the given block is not found, [`ChainSplit::NoSplitPending`] is returned.
    /// Split chain at the number or hash, block with given number will be included at first chain.
    /// If any chain is empty (Does not have blocks) None will be returned.
    ///
    /// # Note
    ///
    /// The plain state is only found in the second chain, making it
    /// impossible to perform any state reverts on the first chain.
    ///
    /// The second chain only contains the changes that were reverted on the first chain; however,
    /// it retains the up to date state as if the chains were one, i.e. the second chain is an
    /// extension of the first.
    #[track_caller]
    pub fn split(mut self, split_at: ChainSplitTarget) -> ChainSplit {
        let chain_tip = *self.blocks.last_entry().expect("chain is never empty").key();
        let block_number = match split_at {
            ChainSplitTarget::Hash(block_hash) => {
                let Some(block_number) = self.block_number(block_hash) else {
                    return ChainSplit::NoSplitPending(self)
                };
                // If block number is same as tip whole chain is becoming canonical.
                if block_number == chain_tip {
                    return ChainSplit::NoSplitCanonical(self)
                }
                block_number
            }
            ChainSplitTarget::Number(block_number) => {
                if block_number >= chain_tip {
                    return ChainSplit::NoSplitCanonical(self)
                }
                if block_number < *self.blocks.first_entry().expect("chain is never empty").key() {
                    return ChainSplit::NoSplitPending(self)
                }
                block_number
            }
        };

        let split_at = block_number + 1;
        let higher_number_blocks = self.blocks.split_off(&split_at);

        let state = std::mem::take(&mut self.state);
        let (canonical_state, pending_state) = state.split_at(split_at);

        // TODO: Currently, trie updates are reset on chain split.
        // Add tests ensuring that it is valid to leave updates in the pending chain.
        ChainSplit::Split {
            canonical: Chain {
                state: canonical_state.expect("split in range"),
                blocks: self.blocks,
                trie_updates: None,
            },
            pending: Chain {
                state: pending_state,
                blocks: higher_number_blocks,
                trie_updates: None,
            },
        }
    }
}

/// Wrapper type for `blocks` display in `Chain`
#[derive(Debug)]
pub struct DisplayBlocksChain<'a>(pub &'a BTreeMap<BlockNumber, SealedBlockWithSenders>);

impl<'a> fmt::Display for DisplayBlocksChain<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.len() <= 3 {
            write!(f, "[")?;
            let mut iter = self.0.values().map(|block| block.num_hash());
            if let Some(block_num_hash) = iter.next() {
                write!(f, "{block_num_hash:?}")?;
                for block_num_hash_iter in iter {
                    write!(f, ", {block_num_hash_iter:?}")?;
                }
            }
            write!(f, "]")?;
        } else {
            write!(
                f,
                "[{:?}, ..., {:?}]",
                self.0.values().next().unwrap().num_hash(),
                self.0.values().last().unwrap().num_hash()
            )?;
        }

        Ok(())
    }
}

/// All blocks in the chain
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChainBlocks<'a> {
    blocks: Cow<'a, BTreeMap<BlockNumber, SealedBlockWithSenders>>,
}

impl<'a> ChainBlocks<'a> {
    /// Creates a consuming iterator over all blocks in the chain with increasing block number.
    ///
    /// Note: this always yields at least one block.
    #[inline]
    pub fn into_blocks(self) -> impl Iterator<Item = SealedBlockWithSenders> {
        self.blocks.into_owned().into_values()
    }

    /// Creates an iterator over all blocks in the chain with increasing block number.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&BlockNumber, &SealedBlockWithSenders)> {
        self.blocks.iter()
    }

    /// Get the tip of the chain.
    ///
    /// # Note
    ///
    /// Chains always have at least one block.
    #[inline]
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain should have at least one block").1
    }

    /// Get the _first_ block of the chain.
    ///
    /// # Note
    ///
    /// Chains always have at least one block.
    #[inline]
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain should have at least one block").1
    }

    /// Returns an iterator over all transactions in the chain.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &TransactionSigned> + '_ {
        self.blocks.values().flat_map(|block| block.body.iter())
    }

    /// Returns an iterator over all transactions and their senders.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.blocks.values().flat_map(|block| block.transactions_with_sender())
    }

    /// Returns an iterator over all [TransactionSignedEcRecovered] in the blocks
    ///
    /// Note: This clones the transactions since it is assumed this is part of a shared [Chain].
    #[inline]
    pub fn transactions_ecrecovered(
        &self,
    ) -> impl Iterator<Item = TransactionSignedEcRecovered> + '_ {
        self.transactions_with_sender().map(|(signer, tx)| tx.clone().with_signer(*signer))
    }

    /// Returns an iterator over all transaction hashes in the block
    #[inline]
    pub fn transaction_hashes(&self) -> impl Iterator<Item = TxHash> + '_ {
        self.blocks.values().flat_map(|block| block.transactions().map(|tx| tx.hash))
    }
}

impl<'a> IntoIterator for ChainBlocks<'a> {
    type Item = (BlockNumber, SealedBlockWithSenders);
    type IntoIter = std::collections::btree_map::IntoIter<BlockNumber, SealedBlockWithSenders>;

    fn into_iter(self) -> Self::IntoIter {
        #[allow(clippy::unnecessary_to_owned)]
        self.blocks.into_owned().into_iter()
    }
}

/// Used to hold receipts and their attachment.
#[derive(Default, Clone, Debug)]
pub struct BlockReceipts {
    /// Block identifier
    pub block: BlockNumHash,
    /// Transaction identifier and receipt.
    pub tx_receipts: Vec<(TxHash, Receipt)>,
}

/// The target block where the chain should be split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainSplitTarget {
    /// Split at block number.
    Number(BlockNumber),
    /// Split at block hash.
    Hash(BlockHash),
}

/// Result of a split chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainSplit {
    /// Chain is not split. Pending chain is returned.
    /// Given block split is higher than last block.
    /// Or in case of split by hash when hash is unknown.
    NoSplitPending(Chain),
    /// Chain is not split. Canonical chain is returned.
    /// Given block split is lower than first block.
    NoSplitCanonical(Chain),
    /// Chain is split into two: `[canonical]` and `[pending]`
    /// The target of this chain split [ChainSplitTarget] belongs to the `canonical` chain.
    Split {
        /// Contains lower block numbers that are considered canonicalized. It ends with
        /// the [ChainSplitTarget] block. The state of this chain is now empty and no longer
        /// usable.
        canonical: Chain,
        /// Right contains all subsequent blocks __after__ the [ChainSplitTarget] that are still
        /// pending.
        ///
        /// The state of the original chain is moved here.
        pending: Chain,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{Receipts, B256};
    use revm::primitives::{AccountInfo, HashMap};

    #[test]
    fn chain_append() {
        let block: SealedBlockWithSenders = SealedBlockWithSenders::default();
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);
        let block3_hash = B256::new([0x03; 32]);
        let block4_hash = B256::new([0x04; 32]);

        let mut block1 = block.clone();
        let mut block2 = block.clone();
        let mut block3 = block.clone();
        let mut block4 = block;

        block1.block.header.set_hash(block1_hash);
        block2.block.header.set_hash(block2_hash);
        block3.block.header.set_hash(block3_hash);
        block4.block.header.set_hash(block4_hash);

        block3.set_parent_hash(block2_hash);

        let mut chain1 =
            Chain { blocks: BTreeMap::from([(1, block1), (2, block2)]), ..Default::default() };

        let chain2 =
            Chain { blocks: BTreeMap::from([(3, block3), (4, block4)]), ..Default::default() };

        assert_eq!(chain1.append_chain(chain2.clone()), Ok(()));

        // chain1 got changed so this will fail
        assert!(chain1.append_chain(chain2).is_err());
    }

    #[test]
    fn test_number_split() {
        let block_state1 = BundleStateWithReceipts::new(
            BundleState::new(
                vec![(
                    Address::new([2; 20]),
                    None,
                    Some(AccountInfo::default()),
                    HashMap::default(),
                )],
                vec![vec![(Address::new([2; 20]), None, vec![])]],
                vec![],
            ),
            Receipts::from_vec(vec![vec![]]),
            1,
        );

        let block_state2 = BundleStateWithReceipts::new(
            BundleState::new(
                vec![(
                    Address::new([3; 20]),
                    None,
                    Some(AccountInfo::default()),
                    HashMap::default(),
                )],
                vec![vec![(Address::new([3; 20]), None, vec![])]],
                vec![],
            ),
            Receipts::from_vec(vec![vec![]]),
            2,
        );

        let mut block1 = SealedBlockWithSenders::default();
        let block1_hash = B256::new([15; 32]);
        block1.set_block_number(1);
        block1.set_hash(block1_hash);
        block1.senders.push(Address::new([4; 20]));

        let mut block2 = SealedBlockWithSenders::default();
        let block2_hash = B256::new([16; 32]);
        block2.set_block_number(2);
        block2.set_hash(block2_hash);
        block2.senders.push(Address::new([4; 20]));

        let mut block_state_extended = block_state1;
        block_state_extended.extend(block_state2);

        let chain = Chain::new(vec![block1.clone(), block2.clone()], block_state_extended, None);

        let (split1_state, split2_state) = chain.state.clone().split_at(2);

        let chain_split1 = Chain {
            state: split1_state.unwrap(),
            blocks: BTreeMap::from([(1, block1.clone())]),
            trie_updates: None,
        };

        let chain_split2 = Chain {
            state: split2_state,
            blocks: BTreeMap::from([(2, block2.clone())]),
            trie_updates: None,
        };

        // return tip state
        assert_eq!(chain.state_at_block(block2.number), Some(chain.state.clone()));
        assert_eq!(chain.state_at_block(block1.number), Some(chain_split1.state.clone()));
        // state at unknown block
        assert_eq!(chain.state_at_block(100), None);

        // split in two
        assert_eq!(
            chain.clone().split(ChainSplitTarget::Hash(block1_hash)),
            ChainSplit::Split { canonical: chain_split1, pending: chain_split2 }
        );

        // split at unknown block hash
        assert_eq!(
            chain.clone().split(ChainSplitTarget::Hash(B256::new([100; 32]))),
            ChainSplit::NoSplitPending(chain.clone())
        );

        // split at higher number
        assert_eq!(
            chain.clone().split(ChainSplitTarget::Number(10)),
            ChainSplit::NoSplitCanonical(chain.clone())
        );

        // split at lower number
        assert_eq!(
            chain.clone().split(ChainSplitTarget::Number(0)),
            ChainSplit::NoSplitPending(chain)
        );
    }
}
