//! Contains [Chain], a chain of blocks and their final state.

use crate::ExecutionOutcome;
use alloc::{borrow::Cow, collections::BTreeMap};
use alloy_eips::{eip1898::ForkBlock, BlockNumHash};
use alloy_primitives::{Address, BlockHash, BlockNumber, TxHash};
use core::{fmt, ops::RangeInclusive};
use reth_execution_errors::{BlockExecutionError, InternalBlockExecutionError};
use reth_primitives::{
    Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, TransactionSigned,
    TransactionSignedEcRecovered,
};
use reth_trie::updates::TrieUpdates;
use revm::db::BundleState;

/// A chain of blocks and their final state.
///
/// The chain contains the state of accounts after execution of its blocks,
/// changesets for those blocks (and their transactions), as well as the blocks themselves.
///
/// Used inside the `BlockchainTree`.
///
/// # Warning
///
/// A chain of blocks should not be empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Chain {
    /// All blocks in this chain.
    blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
    /// The outcome of block execution for this chain.
    ///
    /// This field contains the state of all accounts after the execution of all blocks in this
    /// chain, ranging from the [`Chain::first`] block to the [`Chain::tip`] block, inclusive.
    ///
    /// Additionally, it includes the individual state changes that led to the current state.
    execution_outcome: ExecutionOutcome,
    /// State trie updates after block is added to the chain.
    /// NOTE: Currently, trie updates are present only for
    /// single-block chains that extend the canonical chain.
    trie_updates: Option<TrieUpdates>,
}

impl Chain {
    /// Create new Chain from blocks and state.
    ///
    /// # Warning
    ///
    /// A chain of blocks should not be empty.
    pub fn new(
        blocks: impl IntoIterator<Item = SealedBlockWithSenders>,
        execution_outcome: ExecutionOutcome,
        trie_updates: Option<TrieUpdates>,
    ) -> Self {
        let blocks = BTreeMap::from_iter(blocks.into_iter().map(|b| (b.number, b)));
        debug_assert!(!blocks.is_empty(), "Chain should have at least one block");

        Self { blocks, execution_outcome, trie_updates }
    }

    /// Create new Chain from a single block and its state.
    pub fn from_block(
        block: SealedBlockWithSenders,
        execution_outcome: ExecutionOutcome,
        trie_updates: Option<TrieUpdates>,
    ) -> Self {
        Self::new([block], execution_outcome, trie_updates)
    }

    /// Get the blocks in this chain.
    pub const fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
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
    pub const fn trie_updates(&self) -> Option<&TrieUpdates> {
        self.trie_updates.as_ref()
    }

    /// Remove cached trie updates for this chain.
    pub fn clear_trie_updates(&mut self) {
        self.trie_updates.take();
    }

    /// Get execution outcome of this chain
    pub const fn execution_outcome(&self) -> &ExecutionOutcome {
        &self.execution_outcome
    }

    /// Get mutable execution outcome of this chain
    pub fn execution_outcome_mut(&mut self) -> &mut ExecutionOutcome {
        &mut self.execution_outcome
    }

    /// Prepends the given state to the current state.
    pub fn prepend_state(&mut self, state: BundleState) {
        self.execution_outcome.prepend_state(state);
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

    /// Return execution outcome at the `block_number` or None if block is not known
    pub fn execution_outcome_at_block(
        &self,
        block_number: BlockNumber,
    ) -> Option<ExecutionOutcome> {
        if self.tip().number == block_number {
            return Some(self.execution_outcome.clone())
        }

        if self.blocks.contains_key(&block_number) {
            let mut execution_outcome = self.execution_outcome.clone();
            execution_outcome.revert_to(block_number);
            return Some(execution_outcome)
        }
        None
    }

    /// Destructure the chain into its inner components:
    /// 1. The blocks contained in the chain.
    /// 2. The execution outcome representing the final state.
    /// 3. The optional trie updates.
    pub fn into_inner(self) -> (ChainBlocks<'static>, ExecutionOutcome, Option<TrieUpdates>) {
        (ChainBlocks { blocks: Cow::Owned(self.blocks) }, self.execution_outcome, self.trie_updates)
    }

    /// Destructure the chain into its inner components:
    /// 1. A reference to the blocks contained in the chain.
    /// 2. A reference to the execution outcome representing the final state.
    pub const fn inner(&self) -> (ChainBlocks<'_>, &ExecutionOutcome) {
        (ChainBlocks { blocks: Cow::Borrowed(&self.blocks) }, &self.execution_outcome)
    }

    /// Returns an iterator over all the receipts of the blocks in the chain.
    pub fn block_receipts_iter(&self) -> impl Iterator<Item = &Vec<Option<Receipt>>> + '_ {
        self.execution_outcome.receipts().iter()
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
    ///
    /// # Panics
    ///
    /// If chain doesn't have any blocks.
    #[track_caller]
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain should have at least one block").1
    }

    /// Get the tip of the chain.
    ///
    /// # Panics
    ///
    /// If chain doesn't have any blocks.
    #[track_caller]
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain should have at least one block").1
    }

    /// Returns length of the chain.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Returns the range of block numbers in the chain.
    ///
    /// # Panics
    ///
    /// If chain doesn't have any blocks.
    pub fn range(&self) -> RangeInclusive<BlockNumber> {
        self.first().number..=self.tip().number
    }

    /// Get all receipts for the given block.
    pub fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<&Receipt>> {
        let num = self.block_number(block_hash)?;
        self.execution_outcome.receipts_by_block(num).iter().map(Option::as_ref).collect()
    }

    /// Get all receipts with attachment.
    ///
    /// Attachment includes block number, block hash, transaction hash and transaction index.
    pub fn receipts_with_attachment(&self) -> Vec<BlockReceipts> {
        let mut receipt_attach = Vec::new();
        for ((block_num, block), receipts) in
            self.blocks().iter().zip(self.execution_outcome.receipts().iter())
        {
            let mut tx_receipts = Vec::new();
            for (tx, receipt) in block.body.transactions().zip(receipts.iter()) {
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
        execution_outcome: ExecutionOutcome,
    ) {
        self.blocks.insert(block.number, block);
        self.execution_outcome.extend(execution_outcome);
        self.trie_updates.take(); // reset
    }

    /// Merge two chains by appending the given chain into the current one.
    ///
    /// The state of accounts for this chain is set to the state of the newest chain.
    pub fn append_chain(&mut self, other: Self) -> Result<(), BlockExecutionError> {
        let chain_tip = self.tip();
        let other_fork_block = other.fork_block();
        if chain_tip.hash() != other_fork_block.hash {
            return Err(InternalBlockExecutionError::AppendChainDoesntConnect {
                chain_tip: Box::new(chain_tip.num_hash()),
                other_chain_fork: Box::new(other_fork_block),
            }
            .into())
        }

        // Insert blocks from other chain
        self.blocks.extend(other.blocks);
        self.execution_outcome.extend(other.execution_outcome);
        self.trie_updates.take(); // reset

        Ok(())
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
    ///
    /// # Panics
    ///
    /// If chain doesn't have any blocks.
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
                if block_number > chain_tip {
                    return ChainSplit::NoSplitPending(self)
                }
                if block_number == chain_tip {
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

        let execution_outcome = std::mem::take(&mut self.execution_outcome);
        let (canonical_block_exec_outcome, pending_block_exec_outcome) =
            execution_outcome.split_at(split_at);

        // TODO: Currently, trie updates are reset on chain split.
        // Add tests ensuring that it is valid to leave updates in the pending chain.
        ChainSplit::Split {
            canonical: Self {
                execution_outcome: canonical_block_exec_outcome.expect("split in range"),
                blocks: self.blocks,
                trie_updates: None,
            },
            pending: Self {
                execution_outcome: pending_block_exec_outcome,
                blocks: higher_number_blocks,
                trie_updates: None,
            },
        }
    }
}

/// Wrapper type for `blocks` display in `Chain`
#[derive(Debug)]
pub struct DisplayBlocksChain<'a>(pub &'a BTreeMap<BlockNumber, SealedBlockWithSenders>);

impl fmt::Display for DisplayBlocksChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        let mut values = self.0.values().map(|block| block.num_hash());
        if values.len() <= 3 {
            list.entries(values);
        } else {
            list.entry(&values.next().unwrap());
            list.entry(&format_args!("..."));
            list.entry(&values.next_back().unwrap());
        }
        list.finish()
    }
}

/// All blocks in the chain
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChainBlocks<'a> {
    blocks: Cow<'a, BTreeMap<BlockNumber, SealedBlockWithSenders>>,
}

impl ChainBlocks<'_> {
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
        self.blocks.values().flat_map(|block| block.body.transactions())
    }

    /// Returns an iterator over all transactions and their senders.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &TransactionSigned)> + '_ {
        self.blocks.values().flat_map(|block| block.transactions_with_sender())
    }

    /// Returns an iterator over all [`TransactionSignedEcRecovered`] in the blocks
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

impl IntoIterator for ChainBlocks<'_> {
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

impl From<BlockNumber> for ChainSplitTarget {
    fn from(number: BlockNumber) -> Self {
        Self::Number(number)
    }
}

impl From<BlockHash> for ChainSplitTarget {
    fn from(hash: BlockHash) -> Self {
        Self::Hash(hash)
    }
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
    /// The target of this chain split [`ChainSplitTarget`] belongs to the `canonical` chain.
    Split {
        /// Contains lower block numbers that are considered canonicalized. It ends with
        /// the [`ChainSplitTarget`] block. The state of this chain is now empty and no longer
        /// usable.
        canonical: Chain,
        /// Right contains all subsequent blocks __after__ the [`ChainSplitTarget`] that are still
        /// pending.
        ///
        /// The state of the original chain is moved here.
        pending: Chain,
    },
}

/// Bincode-compatible [`Chain`] serde implementation.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(super) mod serde_bincode_compat {
    use std::collections::BTreeMap;

    use alloc::borrow::Cow;
    use alloy_primitives::BlockNumber;
    use reth_primitives::serde_bincode_compat::SealedBlockWithSenders;
    use reth_trie::serde_bincode_compat::updates::TrieUpdates;
    use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    use crate::ExecutionOutcome;

    /// Bincode-compatible [`super::Chain`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_execution_types::{serde_bincode_compat, Chain};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::Chain")]
    ///     chain: Chain,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct Chain<'a> {
        blocks: SealedBlocksWithSenders<'a>,
        execution_outcome: Cow<'a, ExecutionOutcome>,
        trie_updates: Option<TrieUpdates<'a>>,
    }

    #[derive(Debug)]
    struct SealedBlocksWithSenders<'a>(
        Cow<'a, BTreeMap<BlockNumber, reth_primitives::SealedBlockWithSenders>>,
    );

    impl Serialize for SealedBlocksWithSenders<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut state = serializer.serialize_map(Some(self.0.len()))?;

            for (block_number, block) in self.0.iter() {
                state.serialize_entry(block_number, &SealedBlockWithSenders::<'_>::from(block))?;
            }

            state.end()
        }
    }

    impl<'de> Deserialize<'de> for SealedBlocksWithSenders<'_> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            Ok(Self(Cow::Owned(
                BTreeMap::<BlockNumber, SealedBlockWithSenders<'_>>::deserialize(deserializer)
                    .map(|blocks| blocks.into_iter().map(|(n, b)| (n, b.into())).collect())?,
            )))
        }
    }

    impl<'a> From<&'a super::Chain> for Chain<'a> {
        fn from(value: &'a super::Chain) -> Self {
            Self {
                blocks: SealedBlocksWithSenders(Cow::Borrowed(&value.blocks)),
                execution_outcome: Cow::Borrowed(&value.execution_outcome),
                trie_updates: value.trie_updates.as_ref().map(Into::into),
            }
        }
    }

    impl<'a> From<Chain<'a>> for super::Chain {
        fn from(value: Chain<'a>) -> Self {
            Self {
                blocks: value.blocks.0.into_owned(),
                execution_outcome: value.execution_outcome.into_owned(),
                trie_updates: value.trie_updates.map(Into::into),
            }
        }
    }

    impl SerializeAs<super::Chain> for Chain<'_> {
        fn serialize_as<S>(source: &super::Chain, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Chain::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::Chain> for Chain<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::Chain, D::Error>
        where
            D: Deserializer<'de>,
        {
            Chain::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use arbitrary::Arbitrary;
        use rand::Rng;
        use reth_primitives::SealedBlockWithSenders;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        use super::super::{serde_bincode_compat, Chain};

        #[test]
        fn test_chain_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::Chain")]
                chain: Chain,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let data = Data {
                chain: Chain::new(
                    vec![SealedBlockWithSenders::arbitrary(&mut arbitrary::Unstructured::new(
                        &bytes,
                    ))
                    .unwrap()],
                    Default::default(),
                    None,
                ),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_primitives::{Receipt, Receipts, TxType};
    use revm::primitives::{AccountInfo, HashMap};

    #[test]
    fn chain_append() {
        let block = SealedBlockWithSenders::default();
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

        assert!(chain1.append_chain(chain2.clone()).is_ok());

        // chain1 got changed so this will fail
        assert!(chain1.append_chain(chain2).is_err());
    }

    #[test]
    fn test_number_split() {
        let execution_outcome1 = ExecutionOutcome::new(
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
            vec![vec![]].into(),
            1,
            vec![],
        );

        let execution_outcome2 = ExecutionOutcome::new(
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
            vec![vec![]].into(),
            2,
            vec![],
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

        let mut block_state_extended = execution_outcome1;
        block_state_extended.extend(execution_outcome2);

        let chain = Chain::new(vec![block1.clone(), block2.clone()], block_state_extended, None);

        let (split1_execution_outcome, split2_execution_outcome) =
            chain.execution_outcome.clone().split_at(2);

        let chain_split1 = Chain {
            execution_outcome: split1_execution_outcome.unwrap(),
            blocks: BTreeMap::from([(1, block1.clone())]),
            trie_updates: None,
        };

        let chain_split2 = Chain {
            execution_outcome: split2_execution_outcome,
            blocks: BTreeMap::from([(2, block2.clone())]),
            trie_updates: None,
        };

        // return tip state
        assert_eq!(
            chain.execution_outcome_at_block(block2.number),
            Some(chain.execution_outcome.clone())
        );
        assert_eq!(
            chain.execution_outcome_at_block(block1.number),
            Some(chain_split1.execution_outcome.clone())
        );
        // state at unknown block
        assert_eq!(chain.execution_outcome_at_block(100), None);

        // split in two
        assert_eq!(
            chain.clone().split(block1_hash.into()),
            ChainSplit::Split { canonical: chain_split1, pending: chain_split2 }
        );

        // split at unknown block hash
        assert_eq!(
            chain.clone().split(B256::new([100; 32]).into()),
            ChainSplit::NoSplitPending(chain.clone())
        );

        // split at higher number
        assert_eq!(chain.clone().split(10u64.into()), ChainSplit::NoSplitPending(chain.clone()));

        // split at lower number
        assert_eq!(chain.clone().split(0u64.into()), ChainSplit::NoSplitPending(chain));
    }

    #[test]
    fn receipts_by_block_hash() {
        // Create a default SealedBlockWithSenders object
        let block = SealedBlockWithSenders::default();

        // Define block hashes for block1 and block2
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);

        // Clone the default block into block1 and block2
        let mut block1 = block.clone();
        let mut block2 = block;

        // Set the hashes of block1 and block2
        block1.block.header.set_hash(block1_hash);
        block2.block.header.set_hash(block2_hash);

        // Create a random receipt object, receipt1
        let receipt1 = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
            #[cfg(feature = "optimism")]
            deposit_nonce: Some(18),
            #[cfg(feature = "optimism")]
            deposit_receipt_version: Some(34),
        };

        // Create another random receipt object, receipt2
        let receipt2 = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 1325345,
            logs: vec![],
            success: true,
            #[cfg(feature = "optimism")]
            deposit_nonce: Some(18),
            #[cfg(feature = "optimism")]
            deposit_receipt_version: Some(34),
        };

        // Create a Receipts object with a vector of receipt vectors
        let receipts =
            Receipts { receipt_vec: vec![vec![Some(receipt1.clone())], vec![Some(receipt2)]] };

        // Create an ExecutionOutcome object with the created bundle, receipts, an empty requests
        // vector, and first_block set to 10
        let execution_outcome = ExecutionOutcome {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block: 10,
        };

        // Create a Chain object with a BTreeMap of blocks mapped to their block numbers,
        // including block1_hash and block2_hash, and the execution_outcome
        let chain = Chain {
            blocks: BTreeMap::from([(10, block1), (11, block2)]),
            execution_outcome: execution_outcome.clone(),
            ..Default::default()
        };

        // Assert that the proper receipt vector is returned for block1_hash
        assert_eq!(chain.receipts_by_block_hash(block1_hash), Some(vec![&receipt1]));

        // Create an ExecutionOutcome object with a single receipt vector containing receipt1
        let execution_outcome1 = ExecutionOutcome {
            bundle: Default::default(),
            receipts: Receipts { receipt_vec: vec![vec![Some(receipt1)]] },
            requests: vec![],
            first_block: 10,
        };

        // Assert that the execution outcome at the first block contains only the first receipt
        assert_eq!(chain.execution_outcome_at_block(10), Some(execution_outcome1));

        // Assert that the execution outcome at the tip block contains the whole execution outcome
        assert_eq!(chain.execution_outcome_at_block(11), Some(execution_outcome));
    }
}
