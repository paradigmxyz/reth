//! Handles substate and list of blocks.
//! have functions to split, branch and append the chain.
use crate::{
    execution_result::ExecutionResult,
    executor::Executor,
    substate::{SubStateData, SubStateWithProvider},
};
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{
    BlockHash, BlockNumber, ChainSpec, SealedBlockWithSenders, SealedHeader, U256,
};
use reth_provider::StateProvider;
use reth_revm::database::{State, SubState};
use std::{collections::BTreeMap, sync::Arc};

/// TODO: Chain substate
pub type ChainSubState = bool; //SubState<Box<dyn StateProvider>>;

/// Side chain that contain it state and connect to block found in canonical chain.
#[derive(Clone, Default)]
pub struct Chain {
    /// Chain substate
    pub substate: SubStateData,
    /// Changesets for block and transaction.
    pub changesets: Vec<ExecutionResult>,
    /// Blocks in this chain
    pub blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
}

/// Where does the chain connect to.
/// TODO Should be removed with Chain default. Used for scaffolding.
#[derive(Clone, Copy, Default)]
pub struct ForkBlock {
    /// Block number of block that chains branches from
    pub number: u64,
    /// Block hash of block that chains branches from
    pub hash: BlockHash,
}

/// Chain identification
pub type ChainId = u64;

impl Chain {
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
    pub fn new_canonical_fork<PROVIDER: StateProvider, CONSENSUS: Consensus>(
        block: &SealedBlockWithSenders,
        parent_header: SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        chain_spec: Arc<ChainSpec>,
        provider: &PROVIDER,
        consensus: &CONSENSUS,
    ) -> Result<Self, Error> {
        // verify block against the parent
        consensus.validate_header(block, U256::ZERO)?;
        consensus.pre_validate_header(block, &parent_header)?;
        consensus.pre_validate_block(block)?;

        // substate
        let mut substate = SubStateData::default();
        let empty = BTreeMap::new();

        let unseal = block.clone().split().0.unseal();
        let provider =
            SubStateWithProvider::new(&mut substate, provider, &empty, canonical_block_hashes);
        let mut state_provider = SubState::new(State::new(provider));

        // execute block
        let changesets = vec![Executor::new(chain_spec, &mut state_provider)
            .execute_and_verify_receipt(&unseal, U256::MAX, None)?];

        Ok(Self { substate, changesets, blocks: BTreeMap::from([(block.number, block.clone())]) })
    }

    /// DONE
    /// Create new chain that branches out from existing side chain.
    pub fn new_chain_fork<PROVIDER: StateProvider, CONSENSUS: Consensus>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        chain_spec: Arc<ChainSpec>,
        provider: &PROVIDER,
        consensus: &CONSENSUS,
    ) -> Result<Self, Error> {
        let parent_nubmer = block.number - 1;
        let parent = self
            .blocks
            .get(&parent_nubmer)
            .ok_or(ExecError::BlockNumberNotFoundInChain { block_number: parent_nubmer })?;

        // verify block against the parent
        consensus.validate_header(&block, U256::ZERO)?;
        consensus.pre_validate_header(&block, parent)?;
        consensus.pre_validate_block(&block)?;

        // revert changesets
        let revert_from = self.changesets.len() - (self.tip().number - parent.number) as usize;
        let mut substate = self.substate.clone();

        // Revert changesets to get the state of the parent that we need to apply the change.
        substate.revert(&self.changesets[revert_from..]);

        let provider = SubStateWithProvider::new(
            &mut substate,
            provider,
            &side_chain_block_hashes,
            canonical_block_hashes,
        );

        let unseal = block.clone().split().0.unseal();
        // Create state provider with cached state
        let mut state_provider = SubState::new(State::new(provider));

        // execute block
        let changesets = vec![Executor::new(chain_spec, &mut state_provider)
            .execute_and_verify_receipt(&unseal, U256::MAX, None)?];

        let chain = Self { substate, changesets, blocks: BTreeMap::from([(block.number, block)]) };

        // if all is okay, return new chain back. Present chain is not modified.
        Ok(chain)
    }

    /// Append block to this chain
    pub fn append_block<PROVIDER: StateProvider, CONSENSUS: Consensus>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        chain_spec: Arc<ChainSpec>,
        provider: &PROVIDER,
        consensus: &CONSENSUS,
    ) -> Result<(), Error> {
        let (_, parent) = self.blocks.last_key_value().expect("Chain has at least one block");

        consensus.validate_header(&block, U256::ZERO)?;
        consensus.pre_validate_header(&block, parent)?;
        consensus.pre_validate_block(&block)?;

        // execute block on block substate
        let provider = SubStateWithProvider::new(
            &mut self.substate,
            provider,
            &side_chain_block_hashes,
            canonical_block_hashes,
        );

        let unseal = block.clone().split().0.unseal();
        // Create state provider with cached state
        let mut state_provider = SubState::new(State::new(provider));

        // execute block
        let changeset = Executor::new(chain_spec, &mut state_provider).execute_and_verify_receipt(
            &unseal,
            U256::MAX,
            None,
        )?;

        self.changesets.push(changeset);
        self.blocks.insert(block.number, block);
        Ok(())
    }

    /// Merge two chains into one by appending received chain to the current one.
    /// Take substate from newest one.
    pub fn append_chain(&mut self, chain: Chain) -> bool {
        if self.tip().hash() != chain.last().parent_hash {
            return false
        }

        self.blocks.extend(chain.blocks.into_iter());
        self.changesets.extend(chain.changesets.into_iter());
        self.substate = chain.substate;

        true
    }

    /// Iterate over block to find block with the cache that we want to split on.
    /// Given block cache will be contained in first split. If block with hash
    /// is not found fn would return None.
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
        let first_blocks = self.blocks.split_off(&(block_number + 1));
        let (first_changesets, second_changeset) = self.changesets.split_at(first_blocks.len());

        (
            Some(Chain {
                substate: SubStateData::default(),
                changesets: first_changesets.to_vec(),
                blocks: first_blocks,
            }),
            Some(Chain {
                substate: self.substate,
                changesets: second_changeset.to_vec(),
                blocks: self.blocks,
            }),
        )
    }
}
