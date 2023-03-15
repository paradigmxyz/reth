//! Handles substate and list of blocks.
//! have functions to split, branch and append the chain.
use crate::{
    execution_result::ExecutionResult,
    substate::{SubStateData, SubStateWithProvider},
};
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{BlockHash, BlockNumber, SealedBlockWithSenders, SealedHeader, U256};
use reth_provider::{BlockExecutor, ExecutorFactory, StateProvider};
use std::collections::BTreeMap;

/// Internal to BlockchainTree chain identification.
pub(crate) type BlockChainId = u64;

/// Side chain that contain it state and connect to block found in canonical chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// Chain substate. Updated state after execution all blocks in chain.
    substate: SubStateData,
    /// Changesets for block and transaction. Will be used to update tables in database.
    changesets: Vec<ExecutionResult>,
    /// Blocks in this chain
    blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
}

/// Contains fork block and hash.
#[derive(Clone, Copy)]
pub struct ForkBlock {
    /// Block number of block that chains branches from
    pub number: u64,
    /// Block hash of block that chains branches from
    pub hash: BlockHash,
}

impl ForkBlock {
    /// Return the number hash tuple.
    pub fn num_hash(&self) -> (BlockNumber, BlockHash) {
        (self.number, self.hash)
    }
}

impl Chain {
    /// Return blocks found in chain
    pub fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
        &self.blocks
    }

    /// Into inner components
    pub fn into_inner(
        self,
    ) -> (BTreeMap<BlockNumber, SealedBlockWithSenders>, Vec<ExecutionResult>, SubStateData) {
        (self.blocks, self.changesets, self.substate)
    }

    /// Return execution results of blocks
    pub fn changesets(&self) -> &Vec<ExecutionResult> {
        &self.changesets
    }

    /// Return fork block number and hash.
    pub fn fork_block(&self) -> ForkBlock {
        let tip = self.first();
        ForkBlock { number: tip.number.saturating_sub(1), hash: tip.parent_hash }
    }

    /// Block fork number
    pub fn fork_block_number(&self) -> BlockNumber {
        self.first().number.saturating_sub(1)
    }

    /// Block fork hash
    pub fn fork_block_hash(&self) -> BlockHash {
        self.first().parent_hash
    }

    /// First block in chain.
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain has at least one block for first").1
    }

    /// Return tip of the chain. Chain always have at least one block inside
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.last()
    }

    /// Return tip of the chain. Chain always have at least one block inside
    pub fn last(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain has at least one block for last").1
    }

    /// Create new chain with given blocks and execution result.
    pub fn new(blocks: Vec<(SealedBlockWithSenders, ExecutionResult)>) -> Self {
        let (blocks, changesets): (Vec<_>, Vec<_>) = blocks.into_iter().unzip();

        let blocks = blocks.into_iter().map(|b| (b.number, b)).collect::<BTreeMap<_, _>>();

        let mut substate = SubStateData::default();
        substate.apply(&changesets);

        Self { substate, changesets, blocks }
    }

    /// Create new chain that joins canonical block
    /// If parent block is the tip mark chain fork.
    pub fn new_canonical_fork<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        block: &SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        provider: &SP,
        consensus: &C,
        factory: &EF,
    ) -> Result<Self, Error> {
        // substate
        let substate = SubStateData::default();
        let empty = BTreeMap::new();

        let substate_with_sp =
            SubStateWithProvider::new(&substate, provider, &empty, canonical_block_hashes);

        let changeset = Self::validate_and_execute(
            block.clone(),
            parent_header,
            substate_with_sp,
            consensus,
            factory,
        )?;

        Ok(Self::new(vec![(block.clone(), changeset)]))
    }

    /// Create new chain that branches out from existing side chain.
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

        // revert changesets
        let revert_from = self.changesets.len() - (self.tip().number - parent.number) as usize;
        let mut substate = self.substate.clone();

        // Revert changesets to get the state of the parent that we need to apply the change.
        substate.revert(&self.changesets[revert_from..]);

        let substate_with_sp = SubStateWithProvider::new(
            &substate,
            provider,
            &side_chain_block_hashes,
            canonical_block_hashes,
        );
        let changeset = Self::validate_and_execute(
            block.clone(),
            parent,
            substate_with_sp,
            consensus,
            factory,
        )?;
        substate.apply_one(&changeset);

        let chain = Self {
            substate,
            changesets: vec![changeset],
            blocks: BTreeMap::from([(block.number, block)]),
        };

        // if all is okay, return new chain back. Present chain is not modified.
        Ok(chain)
    }

    /// Validate and execute block and return execution result or error.
    fn validate_and_execute<SP: StateProvider, C: Consensus, EF: ExecutorFactory>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        substate: SubStateWithProvider<'_, SP>,
        consensus: &C,
        factory: &EF,
    ) -> Result<ExecutionResult, Error> {
        consensus.validate_header(&block, U256::MAX)?;
        consensus.pre_validate_header(&block, parent_block)?;
        consensus.pre_validate_block(&block)?;

        let (unseal, senders) = block.into_components();
        let unseal = unseal.unseal();
        let res = factory.with_sp(substate).execute_and_verify_receipt(
            &unseal,
            U256::MAX,
            Some(senders),
        )?;
        Ok(res)
    }

    /// Append block to this chain
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

        let changeset = Self::validate_and_execute(
            block.clone(),
            parent_block,
            SubStateWithProvider::new(
                &self.substate,
                provider,
                &side_chain_block_hashes,
                canonical_block_hashes,
            ),
            consensus,
            factory,
        )?;
        self.substate.apply_one(&changeset);
        self.changesets.push(changeset);
        self.blocks.insert(block.number, block);
        Ok(())
    }

    /// Merge two chains into one by appending received chain to the current one.
    /// Take substate from newest one.
    pub fn append_chain(&mut self, chain: Chain) -> Result<(), Error> {
        let chain_tip = self.tip();
        if chain_tip.hash != chain.fork_block_hash() {
            return Err(ExecError::AppendChainDoesntConnect {
                chain_tip: chain_tip.num_hash(),
                other_chain_fork: chain.fork_block().num_hash(),
            }
            .into())
        }
        self.blocks.extend(chain.blocks.into_iter());
        self.changesets.extend(chain.changesets.into_iter());
        self.substate = chain.substate;
        Ok(())
    }

    /// Split chain at the number or hash, block with given number will be included at first chain.
    /// If any chain is empty (Does not have blocks) None will be returned.
    ///
    /// If block hash is not found ChainSplit::NoSplitPending is returned.
    ///
    /// Subtate state will be only found in second chain. First change substate will be
    /// invalid.
    pub fn split(mut self, split_at: SplitAt) -> ChainSplit {
        let chain_tip = *self.blocks.last_entry().expect("chain is never empty").key();
        let block_number = match split_at {
            SplitAt::Hash(block_hash) => {
                let block_number = self.blocks.iter().find_map(|(num, block)| {
                    if block.hash() == block_hash {
                        Some(*num)
                    } else {
                        None
                    }
                });
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
        let (first_changesets, second_changeset) = self.changesets.split_at(self.blocks.len());

        ChainSplit::Split {
            canonical: Chain {
                substate: SubStateData::default(),
                changesets: first_changesets.to_vec(),
                blocks: self.blocks,
            },
            pending: Chain {
                substate: self.substate,
                changesets: second_changeset.to_vec(),
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
    use crate::substate::AccountSubState;
    use reth_primitives::{H160, H256};
    use reth_provider::execution_result::AccountInfoChangeSet;

    #[test]
    fn chain_apend() {
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

        let mut chain1 = Chain {
            substate: Default::default(),
            changesets: vec![],
            blocks: BTreeMap::from([(1, block1), (2, block2)]),
        };

        let chain2 = Chain {
            substate: Default::default(),
            changesets: vec![],
            blocks: BTreeMap::from([(3, block3), (4, block4)]),
        };

        assert_eq!(chain1.append_chain(chain2.clone()), Ok(()));

        // chain1 got changed so this will fail
        assert!(chain1.append_chain(chain2).is_err());
    }

    #[test]
    fn test_number_split() {
        let mut substate = SubStateData::default();
        let mut account = AccountSubState::default();
        account.info.nonce = 10;
        substate.accounts.insert(H160([1; 20]), account);

        let mut exec1 = ExecutionResult::default();
        exec1.block_changesets.insert(H160([2; 20]), AccountInfoChangeSet::default());
        let mut exec2 = ExecutionResult::default();
        exec2.block_changesets.insert(H160([3; 20]), AccountInfoChangeSet::default());

        let mut block1 = SealedBlockWithSenders::default();
        let block1_hash = H256([15; 32]);
        block1.hash = block1_hash;
        block1.senders.push(H160([4; 20]));

        let mut block2 = SealedBlockWithSenders::default();
        let block2_hash = H256([16; 32]);
        block2.hash = block2_hash;
        block2.senders.push(H160([4; 20]));

        let chain = Chain {
            substate: substate.clone(),
            changesets: vec![exec1.clone(), exec2.clone()],
            blocks: BTreeMap::from([(1, block1.clone()), (2, block2.clone())]),
        };

        let chain_split1 = Chain {
            substate: SubStateData::default(),
            changesets: vec![exec1],
            blocks: BTreeMap::from([(1, block1.clone())]),
        };

        let chain_split2 = Chain {
            substate,
            changesets: vec![exec2.clone()],
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
