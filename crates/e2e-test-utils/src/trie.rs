//! Helpers for verifying the persisted state and trie representation of test nodes.

use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use eyre::{ensure, eyre, Result};
use reth_db::transaction::DbTx;
use reth_provider::{
    BlockNumReader, DBProvider, DatabaseProviderFactory, HeaderProvider, StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    prefix_set::{PrefixSetMut, TriePrefixSets},
    trie_cursor::TrieCursorFactory,
    verify::{default_storage_workers, Output, StorageWorker, Verifier},
    StateRoot,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};

/// Asserts that the persisted state and trie representation is internally consistent and matches
/// the state root of the latest persisted block.
///
/// This performs two checks against persisted database state:
/// - the state root recomputed from the persisted hashed state matches the state root of the latest
///   persisted header, and
/// - the persisted trie nodes match a recomputation from the hashed state (the library equivalent
///   of a `reth db repair-trie --dry-run`).
///
/// Only on-disk data is read, so callers must ensure the blocks of interest have been persisted,
/// e.g. via [`NodeTestContext::wait_for_persisted_block`].
///
/// [`NodeTestContext::wait_for_persisted_block`]: crate::node::NodeTestContext::wait_for_persisted_block
pub fn assert_trie_consistency<F>(factory: &F) -> Result<()>
where
    F: DatabaseProviderFactory<
        Provider: DBProvider + StorageSettingsCache + BlockNumReader + HeaderProvider,
    >,
{
    // Prevent commits while opening the independent snapshots.
    let snapshot_guard = factory.database_provider_rw()?;
    let provider = factory.database_provider_ro()?;
    let tip = provider.best_block_number()?;
    let header = provider
        .header_by_number(tip)?
        .ok_or_else(|| eyre!("missing persisted header for block {tip}"))?;
    let tx = provider.tx_ref();

    reth_trie_db::with_adapter!(provider, |A| {
        let open = || {
            let mut tx = factory.database_provider_ro()?.into_tx();
            tx.disable_long_read_transaction_safety();
            Ok::<_, eyre::Report>(tx)
        };
        let account_tx = open()?;
        let lookahead_tx = open()?;
        let workers = (0..default_storage_workers())
            .map(|_| {
                let tx = open()?;
                Ok::<_, eyre::Report>(StorageWorker {
                    trie: DatabaseTrieCursorFactory::<_, A>::new(&tx)
                        .storage_trie_cursor(B256::ZERO)?,
                    hashed: DatabaseHashedCursorFactory::new(&tx)
                        .hashed_storage_cursor(B256::ZERO)?,
                    transaction: tx,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        drop(snapshot_guard);

        // Recompute all account leaves from the persisted hashed state. Storage roots are taken
        // from the persisted storage tries, which the verifier below checks against the hashed
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

        let verifier = Verifier::new(
            DatabaseTrieCursorFactory::<_, A>::new(&account_tx).account_trie_cursor()?,
            DatabaseHashedCursorFactory::new(&account_tx).hashed_account_cursor()?,
            DatabaseHashedCursorFactory::new(&account_tx).hashed_storage_cursor(B256::ZERO)?,
            DatabaseHashedCursorFactory::new(&lookahead_tx).hashed_account_cursor()?,
            workers,
        )?;
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
