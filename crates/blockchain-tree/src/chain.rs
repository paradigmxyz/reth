//! A chain in a [`BlockchainTree`][super::BlockchainTree].
//!
//! A [`Chain`] contains the state of accounts for the chain after execution of its constituent
//! blocks, as well as a list of the blocks the chain is composed of.

use super::externals::TreeExternals;
use crate::BundleStateDataRef;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{
        error::{BlockchainTreeError, InsertBlockErrorKind},
        BlockAttachment, BlockValidationKind,
    },
    consensus::{Consensus, ConsensusError},
    RethResult,
};
use reth_primitives::{
    BlockHash, BlockNumber, ForkBlock, GotExpected, SealedBlockWithSenders, SealedHeader, U256,
};
use reth_provider::{
    providers::BundleStateProvider, BundleStateDataProvider, BundleStateWithReceipts, Chain,
    ExecutorFactory, StateRootProvider,
};
use reth_trie::updates::TrieUpdates;
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    time::Instant,
};

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
    /// Create a new appendable chain from a given chain.
    pub fn new(chain: Chain) -> Self {
        Self { chain }
    }

    /// Get the chain.
    pub fn into_inner(self) -> Chain {
        self.chain
    }

    /// Create a new chain that forks off of the canonical chain.
    ///
    /// if [BlockValidationKind::Exhaustive] is specified, the method will verify the state root of
    /// the block.
    pub fn new_canonical_fork<DB, EF>(
        block: SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, EF>,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> Result<Self, InsertBlockErrorKind>
    where
        DB: Database,
        EF: ExecutorFactory,
    {
        let state = BundleStateWithReceipts::default();
        let empty = BTreeMap::new();

        let state_provider = BundleStateDataRef {
            state: &state,
            sidechain_block_hashes: &empty,
            canonical_block_hashes,
            canonical_fork,
        };

        let (bundle_state, trie_updates) = Self::validate_and_execute(
            block.clone(),
            parent_header,
            state_provider,
            externals,
            block_attachment,
            block_validation_kind,
        )?;

        Ok(Self { chain: Chain::new(vec![block], bundle_state, trie_updates) })
    }

    /// Create a new chain that forks off of an existing sidechain.
    ///
    /// This differs from [AppendableChain::new_canonical_fork] in that this starts a new fork.
    pub(crate) fn new_chain_fork<DB, EF>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, EF>,
        block_validation_kind: BlockValidationKind,
    ) -> Result<Self, InsertBlockErrorKind>
    where
        DB: Database,
        EF: ExecutorFactory,
    {
        let parent_number = block.number - 1;
        let parent = self.blocks().get(&parent_number).ok_or(
            BlockchainTreeError::BlockNumberNotFoundInChain { block_number: parent_number },
        )?;

        let mut state = self.state().clone();

        // Revert state to the state after execution of the parent block
        state.revert_to(parent.number);

        // Revert changesets to get the state of the parent that we need to apply the change.
        let bundle_state_data = BundleStateDataRef {
            state: &state,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };
        let (block_state, _) = Self::validate_and_execute(
            block.clone(),
            parent,
            bundle_state_data,
            externals,
            BlockAttachment::HistoricalFork,
            block_validation_kind,
        )?;
        // extending will also optimize few things, mostly related to selfdestruct and wiping of
        // storage.
        state.extend(block_state);

        // remove all receipts and reverts (except the last one), as they belong to the chain we
        // forked from and not the new chain we are creating.
        let size = state.receipts().len();
        state.receipts_mut().drain(0..size - 1);
        state.state_mut().take_n_reverts(size - 1);
        state.set_first_block(block.number);

        // If all is okay, return new chain back. Present chain is not modified.
        Ok(Self { chain: Chain::from_block(block, state, None) })
    }

    /// Validate and execute the given block that _extends the canonical chain_, validating its
    /// state root after execution if possible and requested.
    ///
    /// Note: State root validation is limited to blocks that extend the canonical chain and is
    /// optional, see [BlockValidationKind]. So this function takes two parameters to determine
    /// if the state can and should be validated.
    ///   - [BlockAttachment] represents if the block extends the canonical chain, and thus we can
    ///     cache the trie state updates.
    ///   - [BlockValidationKind] determines if the state root __should__ be validated.
    fn validate_and_execute<BSDP, DB, EVM>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        bundle_state_data_provider: BSDP,
        externals: &TreeExternals<DB, EVM>,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> RethResult<(BundleStateWithReceipts, Option<TrieUpdates>)>
    where
        BSDP: BundleStateDataProvider,
        DB: Database,
        EVM: ExecutorFactory,
    {
        // some checks are done before blocks comes here.
        externals.consensus.validate_header_against_parent(&block, parent_block)?;

        // get the state provider.
        let canonical_fork = bundle_state_data_provider.canonical_fork();
        let state_provider =
            externals.provider_factory.history_by_block_number(canonical_fork.number)?;

        let provider = BundleStateProvider::new(state_provider, bundle_state_data_provider);

        let mut executor = externals.executor_factory.with_state(&provider);
        let block_hash = block.hash();
        let block = block.unseal();
        executor.execute_and_verify_receipt(&block, U256::MAX)?;
        let bundle_state = executor.take_output_state();

        // check state root if the block extends the canonical chain __and__ if state root
        // validation was requested.
        if block_validation_kind.is_exhaustive() {
            // calculate and check state root
            let start = Instant::now();
            let (state_root, trie_updates) = if block_attachment.is_canonical() {
                provider
                    .state_root_with_updates(&bundle_state)
                    .map(|(root, updates)| (root, Some(updates)))?
            } else {
                (provider.state_root(&bundle_state)?, None)
            };
            if block.state_root != state_root {
                return Err(ConsensusError::BodyStateRootDiff(
                    GotExpected { got: state_root, expected: block.state_root }.into(),
                )
                .into())
            }

            tracing::debug!(
                target: "blockchain_tree::chain",
                number = block.number,
                hash = %block_hash,
                elapsed = ?start.elapsed(),
                "Validated state root"
            );

            Ok((bundle_state, trie_updates))
        } else {
            Ok((bundle_state, None))
        }
    }

    /// Validate and execute the given block, and append it to this chain.
    ///
    /// This expects that the block's ancestors can be traced back to the `canonical_fork` (the
    /// first parent block of the `block`'s chain that is in the canonical chain).
    ///
    /// In other words, expects a gap less (side-) chain:  [`canonical_fork..block`] in order to be
    /// able to __execute__ the block.
    ///
    /// CAUTION: This will only perform state root check if it's possible: if the `canonical_fork`
    /// is the canonical head, or: state root check can't be performed if the given canonical is
    /// __not__ the canonical head.
    #[track_caller]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_block<DB, EF>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        externals: &TreeExternals<DB, EF>,
        canonical_fork: ForkBlock,
        block_attachment: BlockAttachment,
        block_validation_kind: BlockValidationKind,
    ) -> Result<(), InsertBlockErrorKind>
    where
        DB: Database,
        EF: ExecutorFactory,
    {
        let parent_block = self.chain.tip();

        let bundle_state_data = BundleStateDataRef {
            state: self.state(),
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };

        let (block_state, trie_updates) = Self::validate_and_execute(
            block.clone(),
            parent_block,
            bundle_state_data,
            externals,
            block_attachment,
            block_validation_kind,
        )?;
        // extend the state.
        self.chain.append_block(block, block_state, trie_updates);

        Ok(())
    }
}
