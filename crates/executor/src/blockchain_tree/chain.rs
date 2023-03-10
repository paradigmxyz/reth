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

/// Chain identification
pub type ChainId = u64;

/// Side chain that contain it state and connect to block found in canonical chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// Chain substate
    substate: SubStateData,
    /// Changesets for block and transaction.
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

impl Chain {
    /// Return blocks found in chain
    pub fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
        &self.blocks
    }

    /// Into inneer components
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
        ForkBlock { number: tip.number - 1, hash: tip.parent_hash }
    }

    /// Block fork number
    pub fn fork_block_number(&self) -> BlockNumber {
        self.first().number - 1
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
        consensus.validate_header(&block, U256::ZERO)?;
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
    pub fn append_chain(&mut self, chain: Chain) -> bool {
        if self.tip().hash() != chain.last().parent_hash {
            return false;
        }
        self.blocks.extend(chain.blocks.into_iter());
        self.changesets.extend(chain.changesets.into_iter());
        self.substate = chain.substate;
        true
    }

    /// Iterate over block to find block with the cache that we want to split on.
    /// Given block cache will be contained in first split. If block with hash
    /// is not found first option would be None.
    /// NOTE: Database state will only be found in second chain.
    pub fn split_at_block_hash(self, block_hash: &BlockHash) -> (Option<Chain>, Option<Chain>) {
        let block_number = self.blocks.iter().find_map(|(num, block)| {
            if block.hash() == *block_hash {
                Some(*num)
            } else {
                None
            }
        });
        if let Some(block_number) = block_number {
            Self::split_at_number(self, block_number)
        } else {
            (None, Some(self))
        }
    }

    /// Split chain at the number, block with given number will be included at first chain.
    /// If any chain is empty (Does not have blocks) None will be returned.
    /// NOTE: Subtate state will be only found in second chain. First change substate will be
    /// invalid.
    pub fn split_at_number(mut self, block_number: BlockNumber) -> (Option<Chain>, Option<Chain>) {
        if block_number >= *self.blocks.last_entry().unwrap().key() {
            return (Some(self), None);
        }
        if block_number < *self.blocks.first_entry().unwrap().key() {
            return (None, Some(self));
        }
        let higher_number_blocks = self.blocks.split_off(&(block_number + 1));
        let (first_changesets, second_changeset) = self.changesets.split_at(self.blocks.len());

        (
            Some(Chain {
                substate: SubStateData::default(),
                changesets: first_changesets.to_vec(),
                blocks: self.blocks,
            }),
            Some(Chain {
                substate: self.substate,
                changesets: second_changeset.to_vec(),
                blocks: higher_number_blocks,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::substate::AccountSubState;
    use reth_primitives::{H160, H256};
    use reth_provider::execution_result::AccountInfoChangeSet;

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
        block1.block.header.hash = block1_hash;
        block1.senders.push(H160([4; 20]));

        let mut block2 = SealedBlockWithSenders::default();
        let block2_hash = H256([16; 32]);
        block2.block.header.hash = block2_hash;
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
            chain.clone().split_at_block_hash(&block1_hash),
            (Some(chain_split1), Some(chain_split2))
        );

        // split at unknown block hash
        assert_eq!(
            chain.clone().split_at_block_hash(&H256([100; 32])),
            (None, Some(chain.clone()))
        );

        // split at higher number
        assert_eq!(chain.clone().split_at_number(10), (Some(chain.clone()), None));
        // split at lower number
        assert_eq!(chain.clone().split_at_number(0), (None, Some(chain.clone())));
    }
}
