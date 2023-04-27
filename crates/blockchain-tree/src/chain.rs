//! A chain in a [`BlockchainTree`][super::BlockchainTree].
//!
//! A [`Chain`] contains the state of accounts for the chain after execution of its constituent
//! blocks, as well as a list of the blocks the chain is composed of.
use crate::{post_state::PostState, PostStateDataRef};
use reth_db::database::Database;
use reth_interfaces::{consensus::Consensus, executor::Error as ExecError, Error};
use reth_primitives::{
    BlockHash, BlockNumber, ForkBlock, SealedBlockWithSenders, SealedHeader, U256,
};
use reth_provider::{
    providers::PostStateProvider, BlockExecutor, Chain, ExecutorFactory, PostStateDataProvider,
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

use super::externals::TreeExternals;

/// The ID of a sidechain internally in a [`BlockchainTree`][super::BlockchainTree].
pub(crate) type BlockChainId = u64;

/// A chain if the blockchain tree, that has functionality to execute blocks and append them to the
/// it self.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppendableChain {
    chain: Chain,
}

impl Deref for AppendableChain {
    type Target = Chain;

    fn deref(&self) -> &Self::Target {
        &self.chain
    }
}

impl DerefMut for AppendableChain {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.chain
    }
}

impl AppendableChain {
    /// Crate a new appendable chain from a given chain.
    pub fn new(chain: Chain) -> Self {
        Self { chain }
    }

    /// Get the chain.
    pub fn into_inner(self) -> Chain {
        self.chain
    }

    /// Create a new chain that forks off of the canonical chain.
    pub fn new_canonical_fork<DB, C, EF>(
        block: &SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<Self, Error>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let state = PostState::default();
        let empty = BTreeMap::new();

        let state_provider = PostStateDataRef {
            state: &state,
            sidechain_block_hashes: &empty,
            canonical_block_hashes,
            canonical_fork,
        };

        let changeset =
            Self::validate_and_execute(block.clone(), parent_header, state_provider, externals)?;

        Ok(Self { chain: Chain::new(vec![(block.clone(), changeset)]) })
    }

    /// Create a new chain that forks off of an existing sidechain.
    pub fn new_chain_fork<DB, C, EF>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<Self, Error>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let parent_number = block.number - 1;
        let parent = self
            .blocks()
            .get(&parent_number)
            .ok_or(ExecError::BlockNumberNotFoundInChain { block_number: parent_number })?;

        let mut state = self.state.clone();

        // Revert state to the state after execution of the parent block
        state.revert_to(parent.number);

        // Revert changesets to get the state of the parent that we need to apply the change.
        let post_state_data = PostStateDataRef {
            state: &state,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };
        let block_state =
            Self::validate_and_execute(block.clone(), parent, post_state_data, externals)?;
        state.extend(block_state);

        let chain =
            Self { chain: Chain { state, blocks: BTreeMap::from([(block.number, block)]) } };

        // If all is okay, return new chain back. Present chain is not modified.
        Ok(chain)
    }

    /// Validate and execute the given block.
    fn validate_and_execute<PSDP, DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        post_state_data_provider: PSDP,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<PostState, Error>
    where
        PSDP: PostStateDataProvider,
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        externals.consensus.validate_header_with_total_difficulty(&block, U256::MAX)?;
        externals.consensus.validate_header(&block)?;
        externals.consensus.validate_header_against_parent(&block, parent_block)?;
        externals.consensus.validate_block(&block)?;

        let (unseal, senders) = block.into_components();
        let unseal = unseal.unseal();

        //get state provider.
        let db = externals.shareable_db();
        // TODO, small perf can check if caonical fork is the latest state.
        let canonical_fork = post_state_data_provider.canonical_fork();
        let history_provider = db.history_by_block_number(canonical_fork.number)?;
        let state_provider = history_provider;

        let provider = PostStateProvider::new(state_provider, post_state_data_provider);

        let mut executor = externals.executor_factory.with_sp(&provider);
        executor.execute_and_verify_receipt(&unseal, U256::MAX, Some(senders)).map_err(Into::into)
    }

    /// Validate and execute the given block, and append it to this chain.
    pub fn append_block<DB, C, EF>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<(), Error>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let (_, parent_block) = self.blocks.last_key_value().expect("Chain has at least one block");

        let post_state_data = PostStateDataRef {
            state: &self.state,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };

        let block_state =
            Self::validate_and_execute(block.clone(), parent_block, post_state_data, externals)?;
        self.state.extend(block_state);
        self.blocks.insert(block.number, block);
        Ok(())
    }
}
