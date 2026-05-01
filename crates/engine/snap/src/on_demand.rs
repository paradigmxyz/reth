//! On-demand BAL generation for snap/2 serving.
//!
//! On networks where BALs are not propagated by the consensus layer (e.g. mainnet today), the
//! [`BalStoreHandle`] used by the snap/eth serving paths returns no entries and peers receive
//! the missing-entry sentinel. To enable testing snap/2 against such networks, this module
//! provides [`OnDemandBalStore`], which wraps an inner cache and falls back to building a BAL by
//! replaying the requested block via the EVM whenever the cache misses.
//!
//! Generated BALs are written back to the inner cache so subsequent lookups are cheap.

use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use alloy_rlp::Encodable;
use core::fmt;
use reth_evm::{block::BlockExecutor, ConfigureEvm, Evm};
use reth_primitives_traits::NodePrimitives;
use reth_revm::{database::StateProviderDatabase, State};
use reth_storage_api::{
    BalStore, BalStoreHandle, BlockReader, GetBlockAccessListLimit, StateProviderFactory,
    TransactionVariant,
};
use reth_storage_errors::provider::ProviderResult;
use std::sync::{Arc, OnceLock};

/// Generates a BAL for a block hash by replaying the block.
///
/// Implementations are expected to be cheap to invoke for cache hits in the wrapping store and
/// only do real work on misses. The returned bytes must be the RLP encoding of the block's
/// `Vec<AccountChanges>` (the same payload format embedded in [`BlockAccessLists`]).
///
/// [`BlockAccessLists`]: reth_eth_wire_types::BlockAccessLists
pub trait BalGenerator: Send + Sync + 'static {
    /// Generate the encoded BAL for the given block hash, or `None` if the block is unknown or
    /// generation failed.
    fn generate(&self, block_hash: BlockHash) -> Option<Bytes>;
}

/// A [`BalStore`] that fills cache misses by replaying the requested block via a [`BalGenerator`].
///
/// The generator is injected after construction via [`OnDemandBalStore::set_generator`] so the
/// store can be wired into a provider before the executor / EVM config is available.
pub struct OnDemandBalStore {
    inner: BalStoreHandle,
    generator: OnceLock<Arc<dyn BalGenerator>>,
}

impl OnDemandBalStore {
    /// Create a new on-demand store backed by `inner` for hits and writes.
    pub const fn new(inner: BalStoreHandle) -> Self {
        Self { inner, generator: OnceLock::new() }
    }

    /// Install the generator used for cache misses.
    ///
    /// Calling this more than once is a no-op; the first generator wins.
    pub fn set_generator(&self, generator: Arc<dyn BalGenerator>) {
        let _ = self.generator.set(generator);
    }

    /// Try to produce a BAL for `block_hash`, consulting the inner cache first and otherwise
    /// invoking the generator. Generated BALs are written back to the inner cache.
    fn lookup(&self, block_hash: BlockHash) -> Option<Bytes> {
        if let Ok(mut hits) = self.inner.get_by_hashes(&[block_hash]) &&
            let Some(Some(hit)) = hits.pop()
        {
            return Some(hit);
        }

        let generator = self.generator.get()?;
        let bal = generator.generate(block_hash)?;
        // Best-effort cache fill; failures here are not fatal for serving.
        let _ = self.inner.insert(block_hash, 0, bal.clone());
        Some(bal)
    }
}

impl fmt::Debug for OnDemandBalStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OnDemandBalStore")
            .field("generator_installed", &self.generator.get().is_some())
            .finish_non_exhaustive()
    }
}

impl BalStore for OnDemandBalStore {
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> ProviderResult<()> {
        self.inner.insert(block_hash, block_number, bal)
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        Ok(block_hashes.iter().map(|hash| self.lookup(*hash)).collect())
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let mut size = 0;
        for hash in block_hashes {
            let bal = self.lookup(*hash).unwrap_or_else(|| Bytes::from_static(&[0xc0]));
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break;
            }
        }
        Ok(())
    }

    fn get_by_range(&self, start: BlockNumber, count: u64) -> ProviderResult<Vec<Bytes>> {
        // Range queries are not generated on demand: we do not know which hashes to materialize
        // without first reading the canonical chain, and snap/eth serving paths use hash-based
        // requests. Defer to the inner cache.
        self.inner.get_by_range(start, count)
    }
}

/// [`BalGenerator`] that builds BALs by replaying the block via the configured EVM on top of the
/// parent state.
///
/// Requires a provider that exposes recoverable blocks and historical state at any block hash.
/// On a fully synced node this works for any block whose parent state is still available; on a
/// pruned or snap-syncing node, lookups for blocks outside the available state window will return
/// `None`.
pub struct EvmReplayBalGenerator<P, E> {
    provider: P,
    evm_config: E,
}

impl<P, E> EvmReplayBalGenerator<P, E> {
    /// Create a new generator backed by `provider` and `evm_config`.
    pub const fn new(provider: P, evm_config: E) -> Self {
        Self { provider, evm_config }
    }
}

impl<P, E> fmt::Debug for EvmReplayBalGenerator<P, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmReplayBalGenerator").finish()
    }
}

impl<P, E> BalGenerator for EvmReplayBalGenerator<P, E>
where
    P: StateProviderFactory
        + BlockReader<Block = <E::Primitives as NodePrimitives>::Block>
        + Send
        + Sync
        + 'static,
    E: ConfigureEvm + Send + Sync + 'static,
{
    fn generate(&self, block_hash: BlockHash) -> Option<Bytes> {
        match generate_bal_bytes(&self.provider, &self.evm_config, block_hash) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::debug!(
                    target: "engine::snap::on_demand",
                    %block_hash,
                    %err,
                    "failed to generate BAL on demand",
                );
                None
            }
        }
    }
}

/// Errors that can occur while generating a BAL on demand.
#[derive(Debug, thiserror::Error)]
enum GenerateError {
    #[error(transparent)]
    Provider(#[from] reth_storage_errors::provider::ProviderError),
    #[error("block execution failed: {0}")]
    Execution(String),
    #[error("requested block not found")]
    BlockNotFound,
    #[error("parent state not available")]
    ParentStateMissing,
}

fn generate_bal_bytes<P, E>(
    provider: &P,
    evm_config: &E,
    block_hash: BlockHash,
) -> Result<Option<Bytes>, GenerateError>
where
    P: StateProviderFactory + BlockReader<Block = <E::Primitives as NodePrimitives>::Block>,
    E: ConfigureEvm,
{
    let block = provider
        .recovered_block(BlockHashOrNumber::Hash(block_hash), TransactionVariant::WithHash)?
        .ok_or(GenerateError::BlockNotFound)?;

    let parent_hash = block.parent_hash();
    let state = match provider.state_by_block_hash(parent_hash) {
        Ok(state) => state,
        Err(_) => return Err(GenerateError::ParentStateMissing),
    };

    let mut db = State::builder()
        .with_database(StateProviderDatabase::new(state))
        .with_bal_builder()
        .build();

    let mut executor = evm_config
        .executor_for_block(&mut db, block.sealed_block())
        .map_err(|err| GenerateError::Execution(err.to_string()))?;

    executor
        .apply_pre_execution_changes()
        .map_err(|err| GenerateError::Execution(err.to_string()))?;
    executor.evm_mut().db_mut().bump_bal_index();

    for tx in block.transactions_recovered() {
        executor
            .execute_transaction(tx)
            .map_err(|err| GenerateError::Execution(err.to_string()))?;
        executor.evm_mut().db_mut().bump_bal_index();
    }

    executor
        .apply_post_execution_changes()
        .map_err(|err| GenerateError::Execution(err.to_string()))?;

    let Some(bal) = db.take_built_alloy_bal() else { return Ok(None) };

    // `BlockAccessList` is a type alias for `Vec<AccountChanges>`, whose default RLP encoding is
    // the list payload that the snap/eth wire types embed verbatim (see
    // `crates/net/eth-wire-types/src/block_access_lists.rs`).
    let mut out = Vec::with_capacity(bal.length());
    bal.encode(&mut out);
    Ok(Some(Bytes::from(out)))
}
