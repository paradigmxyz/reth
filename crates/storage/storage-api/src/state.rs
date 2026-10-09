use super::{
    AccountReader, BlockHashReader, BlockIdReader, EvmStateProviderAdapter, StateProofProvider,
    StateRootProvider, StorageRootProvider,
};
use alloc::boxed::Box;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{Address, BlockHash, BlockNumber, StorageKey, StorageValue, B256, U256};
use auto_impl::auto_impl;
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{Bytecode, NodePrimitives};
use reth_storage_errors::provider::{
    CodeChunkError, CodeChunkErrorKind, ProviderError, ProviderResult,
};
use reth_trie_common::HashedPostState;
use revm::database::BundleState;

#[cfg(feature = "chain-state")]
use reth_chain_state::ExecutedBlock;

/// This just receives state, or [`ExecutionOutcome`], from the provider
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StateReader: Send {
    /// Receipt type in [`ExecutionOutcome`].
    type Receipt: Send + Sync;

    /// Get the [`ExecutionOutcome`] for the given block
    fn get_state(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>>;
}

/// Type alias of boxed [`StateProvider`].
pub type StateProviderBox = Box<dyn StateProvider + Send + 'static>;

/// An abstraction for a type that provides state data.
#[auto_impl(&, Arc, Box)]
pub trait StateProvider:
    BlockHashReader
    + AccountReader
    + BytecodeReader
    + StateRootProvider
    + StorageRootProvider
    + StateProofProvider
    + HashedPostStateProvider
{
    /// Get storage of given account.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or has no nonempty code hash.
    /// Missing code referenced by an existing account is a node error.
    fn account_code(&self, addr: &Address) -> ProviderResult<Option<Bytecode>> {
        let Some(acc) = self.basic_account(addr)? else { return Ok(None) };
        let native =
            reth_execution_types::native_provider_account(&acc).map_err(ProviderError::other)?;
        if let Some(target) = native.inline_delegation {
            return Ok(Some(Bytecode(revm::state::Bytecode::new_eip7702(target))));
        }

        if let Some(metadata) = native.code_metadata {
            let stored =
                self.code_chunk_descriptor(&native.code_hash)?.ok_or_else(|| CodeChunkError {
                    code_hash: native.code_hash,
                    index: 0,
                    expected_length: metadata.chunk_len(0),
                    reason: CodeChunkErrorKind::MissingDescriptor,
                    context: None,
                })?;
            if stored.code_size() != metadata.code_size() ||
                stored.chunk_hashes() != metadata.chunk_hashes()
            {
                return Err(CodeChunkError {
                    code_hash: native.code_hash,
                    index: 0,
                    expected_length: metadata.chunk_len(0),
                    reason: CodeChunkErrorKind::DescriptorMismatch,
                    context: None,
                }
                .into())
            }
        }

        if let Some(code_hash) = acc.bytecode_hash {
            if code_hash == KECCAK_EMPTY {
                return Ok(None);
            }
            // Account identity requires code even when representation metadata is unavailable.
            return self.bytecode_by_hash(&code_hash)?.map(Some).ok_or_else(|| {
                CodeChunkError {
                    code_hash,
                    index: 0,
                    expected_length: None,
                    reason: CodeChunkErrorKind::MissingCode,
                    context: None,
                }
                .into()
            });
        }

        // Return `None` if no code hash is set
        Ok(None)
    }

    /// Get account balance by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_balance(&self, addr: &Address) -> ProviderResult<Option<U256>> {
        // Get basic account information
        // Returns None if acc doesn't exist

        self.basic_account(addr)?.map_or_else(|| Ok(None), |acc| Ok(Some(acc.balance)))
    }

    /// Get account nonce by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_nonce(&self, addr: &Address) -> ProviderResult<Option<u64>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        self.basic_account(addr)?.map_or_else(|| Ok(None), |acc| Ok(Some(acc.nonce)))
    }

    /// Wraps this provider for EVM execution without allocating or cloning it.
    ///
    /// Call this on a reference to borrow the provider, or on a box to retain ownership.
    #[auto_impl(keep_default_for(&, Arc, Box))]
    fn into_evm_state_provider(self) -> EvmStateProviderAdapter<Self>
    where
        Self: Sized,
    {
        EvmStateProviderAdapter(self)
    }
}

/// Minimal requirements to read a full account, for example, to validate its new transactions
pub trait AccountInfoReader: AccountReader + BytecodeReader {}
impl<T: AccountReader + BytecodeReader> AccountInfoReader for T {}

/// Trait that provides the hashed state from various sources.
#[auto_impl(&, Arc, Box)]
pub trait HashedPostStateProvider {
    /// Returns the [`HashedPostState`] of the provided [`BundleState`], materializing zero-valued
    /// updates for parent storage of accounts that were destroyed but remain in the post-state.
    ///
    /// Providers backed by an exact parent-state view also materialize terminally destroyed
    /// accounts with explicit zero-valued storage updates.
    fn hashed_post_state(&self, bundle_state: &BundleState) -> ProviderResult<HashedPostState>;
}

/// Trait for reading bytecode associated with a given code hash.
#[auto_impl(&, Arc, Box)]
pub trait BytecodeReader {
    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>>;

    /// Stored legacy code kind, where true denotes delegation and absence denotes no row.
    /// This operation must not load or analyze a complete runtime payload.
    fn legacy_code_kind(&self, _hash: &B256) -> ProviderResult<Option<bool>> {
        Err(ProviderError::UnsupportedProvider)
    }

    /// Retrieve a historical whole-code record, without consulting chunk descriptors.
    /// Absence does not cause a full-code fetch through a sparse API.
    fn legacy_bytecode_by_hash(&self, _hash: &B256) -> ProviderResult<Option<Bytecode>> {
        Ok(None)
    }

    /// Read an old-format delegation marker without copying ordinary runtime payloads.
    fn legacy_delegation(&self, _code_hash: &B256) -> ProviderResult<Option<Address>> {
        Ok(None)
    }

    /// Read bounded execution preparation for explicitly chunked code.
    /// Legacy-only providers return absence; never reconstruct full code here.
    fn code_chunk_descriptor(
        &self,
        _code_hash: &B256,
    ) -> ProviderResult<Option<crate::CodeChunkDescriptor>> {
        Ok(None)
    }

    /// Fetch one original payload without reconstructing complete code.
    /// Providers without sparse support explicitly reject this operation.
    fn get_code_chunk_by_hash(
        &self,
        _code_hash: &B256,
        _index: u32,
    ) -> ProviderResult<Option<alloy_primitives::Bytes>> {
        Err(reth_storage_errors::provider::ProviderError::UnsupportedProvider)
    }

    /// Read content required by an account's committed representation.
    fn get_required_code_chunk(
        &self,
        code_hash: &B256,
        representation: &crate::CodeRepresentation,
        index: u32,
    ) -> ProviderResult<Option<alloy_primitives::Bytes>> {
        match representation {
            crate::CodeRepresentation::Empty => Ok(None),
            crate::CodeRepresentation::Legacy if index != 0 => Ok(None),
            crate::CodeRepresentation::Chunked(descriptor)
                if descriptor.chunk_range(index).is_none() =>
            {
                Ok(None)
            }
            _ => {
                let _ = code_hash;
                Err(reth_storage_errors::provider::ProviderError::UnsupportedProvider)
            }
        }
    }

    /// Preserve storage failure details and attach available execution context.
    fn get_required_code_chunk_with_context(
        &self,
        code_hash: &B256,
        representation: &crate::CodeRepresentation,
        index: u32,
        context: Option<crate::CodeReadContext>,
    ) -> ProviderResult<Option<alloy_primitives::Bytes>> {
        self.get_required_code_chunk(code_hash, representation, index).map_err(|mut error| {
            if let reth_storage_errors::provider::ProviderError::CodeChunk(chunk) = &mut error {
                chunk.context = context;
            }
            error
        })
    }
}

/// Light wrapper that returns `StateProvider` implementations that correspond to the given
/// `BlockNumber`, the latest state, or the pending state.
///
/// This type differentiates states into `historical`, `latest` and `pending`, where the `latest`
/// block determines what is historical or pending: `[historical..latest..pending]`.
///
/// The `latest` state represents the state after the most recent block has been committed to the
/// database, `historical` states are states that have been committed to the database before the
/// `latest` state, and `pending` states are states that have not yet been committed to the
/// database which may or may not become the `latest` state, depending on consensus.
///
/// Note: the `pending` block is considered the block that extends the canonical chain but one and
/// has the `latest` block as its parent.
///
/// All states are _inclusive_, meaning they include _all_ changes made (executed transactions)
/// in their respective blocks. For example [`StateProviderFactory::history_by_block_number`] for
/// block number `n` will return the state after block `n` was executed (transactions, withdrawals).
/// In other words, all states point to the end of the state's respective block, which is equivalent
/// to state at the beginning of the child block.
///
/// This affects tracing, or replaying blocks, which will need to be executed on top of the state of
/// the parent block. For example, in order to trace block `n`, the state after block `n - 1` needs
/// to be used, since block `n` was executed on its parent block's state.
#[auto_impl(&, Box, Arc)]
pub trait StateProviderFactory: BlockIdReader + Send {
    /// The node primitive types.
    type Primitives: NodePrimitives;

    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox>;

    /// Returns a state provider after applying `block` to `parent_hash`.
    #[cfg(feature = "chain-state")]
    fn state_with_block_appended(
        &self,
        parent_hash: BlockHash,
        block: ExecutedBlock<Self::Primitives>,
    ) -> ProviderResult<StateProviderBox>;

    /// Returns a [`StateProvider`] indexed by the given [`BlockId`].
    ///
    /// Note: if a number or hash is provided this will __only__ look at historical(canonical)
    /// state.
    fn state_by_block_id(&self, block_id: BlockId) -> ProviderResult<StateProviderBox> {
        match block_id {
            BlockId::Number(block_number) => self.state_by_block_number_or_tag(block_number),
            BlockId::Hash(block_hash) => self.history_by_block_hash(block_hash.into()),
        }
    }

    /// Returns a [`StateProvider`] indexed by the given block number or tag.
    ///
    /// Note: if a number is provided this will only look at historical(canonical) state.
    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox>;

    /// Returns a historical [`StateProvider`] indexed by the given historic block number.
    ///
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox>;

    /// Returns a historical [`StateProvider`] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox>;

    /// Returns _any_ [StateProvider] with matching block hash.
    ///
    /// This will return a [StateProvider] for either a historical or pending block.
    fn state_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox>;

    /// Storage provider for pending state.
    ///
    /// Represents the state at the block that extends the canonical chain by one.
    /// If there's no `pending` block, then this is equal to [`StateProviderFactory::latest`]
    fn pending(&self) -> ProviderResult<StateProviderBox>;

    /// Storage provider for pending state for the given block hash.
    ///
    /// Represents the state at the block that extends the canonical chain.
    ///
    /// If the block couldn't be found, returns `None`.
    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>>;

    /// Returns a pending [`StateProvider`] if it exists.
    ///
    /// This will return `None` if there's no pending state.
    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>>;
}

/// Sparse and full bytecode access share the same provider identity.
pub use BytecodeReader as CodeChunkReader;

/// Select an original payload from code already resident in an execution overlay or cache.
/// This helper never fetches code. Multi-chunk resident input is authenticated before use.
pub fn resident_code_chunk(
    hash: &B256,
    bytes: alloy_primitives::Bytes,
    representation: Option<&crate::CodeRepresentation>,
    index: u32,
) -> ProviderResult<Option<alloy_primitives::Bytes>> {
    if matches!(representation, Some(crate::CodeRepresentation::Empty)) ||
        *hash == alloy_primitives::keccak256([])
    {
        return Ok(None);
    }
    if matches!(representation, Some(crate::CodeRepresentation::Legacy)) {
        if index != 0 {
            return Ok(None);
        }
        if bytes.len() > crate::LEGACY_CODE_CHUNK_SIZE {
            return Err(CodeChunkError {
                code_hash: *hash,
                index,
                expected_length: None,
                reason: CodeChunkErrorKind::UnsupportedLegacySize { actual: bytes.len() },
                context: None,
            }
            .into());
        }
        return Ok(Some(bytes));
    }
    if let Some(crate::CodeRepresentation::Chunked(descriptor)) = representation &&
        descriptor.chunk_range(index).is_none()
    {
        return Ok(None);
    }
    let code = crate::ValidatedCode::new(bytes).map_err(ProviderError::InvalidChunkedCode)?;
    if code.code_hash() != *hash {
        return Err(CodeChunkError {
            code_hash: *hash,
            index,
            expected_length: None,
            reason: CodeChunkErrorKind::FullCodeHashMismatch,
            context: None,
        }
        .into());
    }
    if let Some(crate::CodeRepresentation::Chunked(descriptor)) = representation &&
        !code.descriptor().is_some_and(|prepared| prepared.same_commitment(descriptor))
    {
        return Err(CodeChunkError {
            code_hash: *hash,
            index,
            expected_length: descriptor.chunk_range(index).map(|range| range.len()),
            reason: CodeChunkErrorKind::DescriptorMismatch,
            context: None,
        }
        .into());
    }
    Ok(code.chunks().get(index as usize).cloned())
}
