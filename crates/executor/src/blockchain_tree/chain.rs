//! A chain in a [`BlockchainTree`][super::BlockchainTree].
//!
//! A [`Chain`] contains the state of accounts for the chain after execution of its constituent
//! blocks, as well as a list of the blocks the chain is composed of.
use crate::{post_state::PostState, substate::PostStateProvider};
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{BlockHash, BlockNumber, SealedBlockWithSenders, SealedHeader, U256};
use reth_provider::{BlockExecutor, ExecutorFactory, StateProvider};
use std::collections::BTreeMap;

/// The ID of a sidechain internally in a [`BlockchainTree`][super::BlockchainTree].
pub(crate) type BlockChainId = u64;

/// A side chain.
///
/// The sidechain contains the state of accounts after execution of its blocks,
/// changesets for those blocks (and their transactions), as well as the blocks themselves.
///
/// Each chain in the tree are identified using a unique ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// The state of accounts after execution of the blocks in this chain.
    ///
    /// This state also contains the individual changes that lead to the current state.
    state: PostState,
    /// The blocks in this chain.
    blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
    /// A mapping of each block number in the chain to the highest transition ID in the chain's
    /// state after execution of the block.
    ///
    /// This is used to revert changes in the state until a certain block number when the chain is
    /// split.
    block_transitions: BTreeMap<BlockNumber, usize>,
}

/// Describes a fork block by its number and hash.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ForkBlock {
    /// Block number of block that chains branches from
    pub number: u64,
    /// Block hash of block that chains branches from
    pub hash: BlockHash,
}

impl ForkBlock {
    /// Return the `(block_number, block_hash)` tuple for this fork block.
    pub fn num_hash(&self) -> (BlockNumber, BlockHash) {
        (self.number, self.hash)
    }
}

impl Chain {
    /// Get the blocks in this chain.
    pub fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
        &self.blocks
    }

    /// Destructure the chain into its inner components, the blocks and the state.
    pub fn into_inner(self) -> (BTreeMap<BlockNumber, SealedBlockWithSenders>, PostState) {
        (self.blocks, self.state)
    }

    /// Get the block at which this chain forked.
    pub fn fork_block(&self) -> ForkBlock {
        let tip = self.first();
        ForkBlock { number: tip.number.saturating_sub(1), hash: tip.parent_hash }
    }

    /// Get the block number at which this chain forked.
    pub fn fork_block_number(&self) -> BlockNumber {
        self.first().number.saturating_sub(1)
    }

    /// Get the block hash at which this chain forked.
    pub fn fork_block_hash(&self) -> BlockHash {
        self.first().parent_hash
    }

    /// Get the first block in this chain.
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain has at least one block for first").1
    }

    /// Get the tip of the chain.
    ///
    /// # Note
    ///
    /// Chains always have at least one block.
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain should have at least one block").1
    }

    /// Create new chain with given blocks and post state.
    pub fn new(blocks: Vec<(SealedBlockWithSenders, PostState)>) -> Self {
        let mut state = PostState::default();
        let mut block_transitions = BTreeMap::new();
        let mut block_num_hash = BTreeMap::new();
        for (block, block_state) in blocks.into_iter() {
            state.extend(block_state);
            block_transitions.insert(block.number, state.transitions_count());
            block_num_hash.insert(block.number, block);
        }

        Self { state, block_transitions, blocks: block_num_hash }
    }

    /// Create a new chain that forks off of the canonical chain.
    pub fn new_canonical_fork<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        block: &SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        provider: &SP,
        consensus: &C,
        factory: &EF,
    ) -> Result<Self, Error> {
        let state = PostState::default();
        let empty = BTreeMap::new();

        let state_provider =
            PostStateProvider::new(&state, provider, &empty, canonical_block_hashes);

        let changeset = Self::validate_and_execute(
            block.clone(),
            parent_header,
            state_provider,
            consensus,
            factory,
        )?;

        Ok(Self::new(vec![(block.clone(), changeset)]))
    }

    /// Create a new chain that forks off of an existing sidechain.
    pub fn new_chain_fork<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        provider: &SP,
        consensus: &C,
        factory: &EF,
    ) -> Result<Self, Error> {
        let parent_number = block.number - 1;
        let parent = self
            .blocks
            .get(&parent_number)
            .ok_or(ExecError::BlockNumberNotFoundInChain { block_number: parent_number })?;

        let revert_to_transition_id = self
            .block_transitions
            .get(&parent.number)
            .expect("Should have the transition ID for the parent block");
        let mut state = self.state.clone();

        // Revert state to the state after execution of the parent block
        state.revert_to(*revert_to_transition_id);

        // Revert changesets to get the state of the parent that we need to apply the change.
        let state_provider = PostStateProvider::new(
            &state,
            provider,
            &side_chain_block_hashes,
            canonical_block_hashes,
        );
        let block_state =
            Self::validate_and_execute(block.clone(), parent, state_provider, consensus, factory)?;
        state.extend(block_state);

        let chain = Self {
            block_transitions: BTreeMap::from([(block.number, state.transitions_count())]),
            state,
            blocks: BTreeMap::from([(block.number, block)]),
        };

        // If all is okay, return new chain back. Present chain is not modified.
        Ok(chain)
    }

    /// Validate and execute the given block.
    fn validate_and_execute<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        state_provider: PostStateProvider<'_, SP>,
        consensus: &C,
        factory: &EF,
    ) -> Result<PostState, Error> {
        consensus.validate_header(&block, U256::MAX)?;
        consensus.pre_validate_header(&block, parent_block)?;
        consensus.pre_validate_block(&block)?;

        let (unseal, senders) = block.into_components();
        let unseal = unseal.unseal();

        factory
            .with_sp(state_provider)
            .execute_and_verify_receipt(&unseal, U256::MAX, Some(senders))
            .map_err(Into::into)
    }

    /// Validate and execute the given block, and append it to this chain.
    pub fn append_block<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        provider: &SP,
        consensus: &C,
        factory: &EF,
    ) -> Result<(), Error> {
        let (_, parent_block) = self.blocks.last_key_value().expect("Chain has at least one block");

        let block_state = Self::validate_and_execute(
            block.clone(),
            parent_block,
            PostStateProvider::new(
                &self.state,
                provider,
                &side_chain_block_hashes,
                canonical_block_hashes,
            ),
            consensus,
            factory,
        )?;
        self.state.extend(block_state);
        self.block_transitions.insert(block.number, self.state.transitions_count());
        self.blocks.insert(block.number, block);
        Ok(())
    }

    /// Merge two chains by appending the given chain into the current one.
    ///
    /// The state of accounts for this chain is set to the state of the newest chain.
    pub fn append_chain(&mut self, chain: Chain) -> Result<(), Error> {
        let chain_tip = self.tip();
        if chain_tip.hash != chain.fork_block_hash() {
            return Err(ExecError::AppendChainDoesntConnect {
                chain_tip: chain_tip.num_hash(),
                other_chain_fork: chain.fork_block().num_hash(),
            }
            .into())
        }

        // Insert blocks from other chain
        self.blocks.extend(chain.blocks.into_iter());
        let current_transition_count = self.state.transitions_count();
        self.state.extend(chain.state);

        // Update the block transition mapping, shifting the transition ID by the current number of
        // transitions in *this* chain
        for (block_number, transition_id) in chain.block_transitions.iter() {
            self.block_transitions.insert(*block_number, transition_id + current_transition_count);
        }
        Ok(())
    }

    /// Split this chain at the given block.
    ///
    /// The given block will be the first block in the first returned chain.
    ///
    /// If the given block is not found, [`ChainSplit::NoSplitPending`] is returned.
    /// Split chain at the number or hash, block with given number will be included at first chain.
    /// If any chain is empty (Does not have blocks) None will be returned.
    ///
    /// # Note
    ///
    /// The block number to transition ID mapping is only found in the second chain, making it
    /// impossible to perform any state reverts on the first chain.
    ///
    /// The second chain only contains the changes that were reverted on the first chain; however,
    /// it retains the up to date state as if the chains were one, i.e. the second chain is an
    /// extension of the first.
    pub fn split(mut self, split_at: SplitAt) -> ChainSplit {
        let chain_tip = *self.blocks.last_entry().expect("chain is never empty").key();
        let block_number = match split_at {
            SplitAt::Hash(block_hash) => {
                let block_number = self
                    .blocks
                    .iter()
                    .find_map(|(num, block)| (block.hash() == block_hash).then_some(*num));
                let Some(block_number) = block_number else { return ChainSplit::NoSplitPending(self)};
                // If block number is same as tip whole chain is becoming canonical.
                if block_number == chain_tip {
                    return ChainSplit::NoSplitCanonical(self)
                }
                block_number
            }
            SplitAt::Number(block_number) => {
                if block_number >= chain_tip {
                    return ChainSplit::NoSplitCanonical(self)
                }
                if block_number < *self.blocks.first_entry().expect("chain is never empty").key() {
                    return ChainSplit::NoSplitPending(self)
                }
                block_number
            }
        };

        let higher_number_blocks = self.blocks.split_off(&(block_number + 1));

        let mut canonical_state = std::mem::take(&mut self.state);
        let new_state = canonical_state.split_at(
            *self.block_transitions.get(&(block_number)).expect("Unknown block transition ID"),
        );
        self.state = new_state;

        ChainSplit::Split {
            canonical: Chain {
                state: canonical_state,
                block_transitions: BTreeMap::new(),
                blocks: self.blocks,
            },
            pending: Chain {
                state: self.state,
                block_transitions: self.block_transitions,
                blocks: higher_number_blocks,
            },
        }
    }
}

/// Used in spliting the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAt {
    /// Split at block number.
    Number(BlockNumber),
    /// Split at block hash.
    Hash(BlockHash),
}

/// Result of spliting chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainSplit {
    /// Chain is not splited. Pending chain is returned.
    /// Given block split is higher than last block.
    /// Or in case of split by hash when hash is unknown.
    NoSplitPending(Chain),
    /// Chain is not splited. Canonical chain is returned.
    /// Given block split is lower than first block.
    NoSplitCanonical(Chain),
    /// Chain is splited in two.
    /// Given block split is contained in first chain.
    Split {
        /// Left contains lower block number that get canonicalized.
        /// And substate is empty and not usable.
        canonical: Chain,
        /// Right contains higher block number, that is still pending.
        /// And substate from original chain is moved here.
        pending: Chain,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{Account, H160, H256};

    #[test]
    fn chain_append() {
        let block = SealedBlockWithSenders::default();
        let block1_hash = H256([0x01; 32]);
        let block2_hash = H256([0x02; 32]);
        let block3_hash = H256([0x03; 32]);
        let block4_hash = H256([0x04; 32]);

        let mut block1 = block.clone();
        let mut block2 = block.clone();
        let mut block3 = block.clone();
        let mut block4 = block;

        block1.block.header.hash = block1_hash;
        block2.block.header.hash = block2_hash;
        block3.block.header.hash = block3_hash;
        block4.block.header.hash = block4_hash;

        block3.block.header.header.parent_hash = block2_hash;

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
        let mut base_state = PostState::default();
        let mut account = Account::default();
        account.nonce = 10;
        base_state.create_account(H160([1; 20]), account);
        base_state.finish_transition();

        let mut block_state1 = PostState::default();
        block_state1.create_account(H160([2; 20]), Account::default());
        block_state1.finish_transition();

        let mut block_state2 = PostState::default();
        block_state2.create_account(H160([3; 20]), Account::default());
        block_state2.finish_transition();

        let mut block1 = SealedBlockWithSenders::default();
        let block1_hash = H256([15; 32]);
        block1.number = 1;
        block1.hash = block1_hash;
        block1.senders.push(H160([4; 20]));

        let mut block2 = SealedBlockWithSenders::default();
        let block2_hash = H256([16; 32]);
        block2.number = 2;
        block2.hash = block2_hash;
        block2.senders.push(H160([4; 20]));

        let chain = Chain::new(vec![
            (block1.clone(), block_state1.clone()),
            (block2.clone(), block_state2.clone()),
        ]);

        let mut split1_state = chain.state.clone();
        let split2_state = split1_state.split_at(*chain.block_transitions.get(&1).unwrap());

        let chain_split1 = Chain {
            state: split1_state,
            block_transitions: BTreeMap::new(),
            blocks: BTreeMap::from([(1, block1.clone())]),
        };

        let chain_split2 = Chain {
            state: split2_state,
            block_transitions: chain.block_transitions.clone(),
            blocks: BTreeMap::from([(2, block2.clone())]),
        };

        // split in two
        assert_eq!(
            chain.clone().split(SplitAt::Hash(block1_hash)),
            ChainSplit::Split { canonical: chain_split1, pending: chain_split2 }
        );

        // split at unknown block hash
        assert_eq!(
            chain.clone().split(SplitAt::Hash(H256([100; 32]))),
            ChainSplit::NoSplitPending(chain.clone())
        );

        // split at higher number
        assert_eq!(
            chain.clone().split(SplitAt::Number(10)),
            ChainSplit::NoSplitCanonical(chain.clone())
        );

        // split at lower number
        assert_eq!(chain.clone().split(SplitAt::Number(0)), ChainSplit::NoSplitPending(chain));
    }
}
