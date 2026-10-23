//! Helpers for verifying the persisted state and trie representation of test nodes.

use alloy_consensus::BlockHeader;
use eyre::{ensure, eyre, Result};
use reth_provider::{
    BlockNumReader, DBProvider, DatabaseProviderFactory, HeaderProvider, StorageSettingsCache,
};
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    verify::{Output, Verifier},
    StateRoot,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};

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
/// [`NodeTestContext::wait_for_persisted_block`]: crate::node::NodeTestContext::wait_for_persisted_block
pub fn assert_trie_consistency<F>(factory: &F) -> Result<()>
where
    F: DatabaseProviderFactory<
        Provider: DBProvider + StorageSettingsCache + BlockNumReader + HeaderProvider,
    >,
{
    let provider = factory.database_provider_ro()?;
    let tip = provider.best_block_number()?;
    let header = provider
        .header_by_number(tip)?
        .ok_or_else(|| eyre!("missing persisted header for block {tip}"))?;
    let tx = provider.tx_ref();

    reth_trie_db::with_adapter!(provider, |A| {
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

        let trie_cursor_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
        let verifier = Verifier::new(&trie_cursor_factory, DatabaseHashedCursorFactory::new(tx))?;
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
