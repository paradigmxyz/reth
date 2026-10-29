//! Helpers for verifying the persisted state and trie representation of test nodes.

use alloy_consensus::BlockHeader;
use eyre::{ensure, eyre, Result};
use reth_provider::{
    BlockNumReader, DBProvider, DatabaseProviderFactory, HeaderProvider, StorageSettingsCache,
};
use reth_tasks::TaskExecutor;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    verify::Output,
    StateRoot,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::verify::{DatabaseVerifier, DEFAULT_STORAGE_LOOKAHEAD};
use std::num::NonZeroUsize;

/// Asserts that the persisted state and trie representation is internally consistent and matches
/// the state root of the latest persisted block.
///
/// This performs two checks against a single database snapshot:
/// - the state root recomputed from the persisted hashed state matches the state root of the latest
///   persisted header, and
/// - the persisted trie nodes match a recomputation from the hashed state (the library equivalent
///   of a `reth db repair-trie --dry-run`).
///
/// Only on-disk data is read, so callers must ensure the blocks of interest have been persisted,
/// e.g. via [`NodeTestContext::wait_for_persisted_block`].
///
/// This briefly acquires the database write lock to open a consistent set of read transactions,
/// blocking node persistence until they are open. Do not call while holding a write transaction:
/// acquiring the same lock again would deadlock. Verification then releases the write lock but
/// retains read snapshots until both checks complete.
///
/// [`NodeTestContext::wait_for_persisted_block`]: crate::node::NodeTestContext::wait_for_persisted_block
pub fn assert_trie_consistency<F>(factory: &F, task_executor: &TaskExecutor) -> Result<()>
where
    F: DatabaseProviderFactory<
        Provider: DBProvider + StorageSettingsCache + BlockNumReader + HeaderProvider,
    >,
{
    // Prevent commits while opening the independent snapshots.
    let snapshot_guard = factory.database_provider_rw()?;
    let provider = factory.database_provider_ro()?.disable_long_read_transaction_safety();
    let tip = provider.best_block_number()?;
    let header = provider
        .header_by_number(tip)?
        .ok_or_else(|| eyre!("missing persisted header for block {tip}"))?;
    let tx = provider.tx_ref();

    reth_trie_db::with_adapter!(provider, |A| {
        let verifier = DatabaseVerifier::from_provider_factory::<_, A>(
            factory,
            task_executor,
            NonZeroUsize::new(task_executor.cpu_pool().current_num_threads())
                .unwrap_or(NonZeroUsize::MIN),
            DEFAULT_STORAGE_LOOKAHEAD,
        )?;
        drop(snapshot_guard);

        // Recompute all account leaves from the persisted hashed state. Storage roots are taken
        // from the persisted storage tries, which the verifier checks against the hashed
        // storage tables.
        let recomputed = StateRoot::new(
            DatabaseTrieCursorFactory::<_, A>::new(tx),
            DatabaseHashedCursorFactory::new(tx),
        )
        .with_prefix_sets(TriePrefixSets {
            account_prefix_set: PrefixSetMut::all().freeze(),
            ..Default::default()
        })
        .root()?;
        ensure!(
            recomputed == header.state_root(),
            "state root {recomputed} recomputed from the persisted state does not match the state root of persisted block {tip}: {}",
            header.state_root()
        );

        let mut inconsistencies = Vec::new();
        for output in verifier {
            match output? {
                Output::Progress(_) => {}
                inconsistency => inconsistencies.push(inconsistency),
            }
        }
        ensure!(
            inconsistencies.is_empty(),
            "persisted trie is inconsistent with the persisted hashed state: {inconsistencies:?}"
        );

        Ok(())
    })
}
