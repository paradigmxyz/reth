//! Finality-gated MPT disposal, committed atomically with a permanent database marker.

use alloy_primitives::B256;
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_engine_tree::tree::state_root_strategy::pbt::PbtStateRootStrategy;
use reth_primitives_traits::{AlloyBlockHeader, BlockHeader, SealedHeader};
use reth_provider::{
    BlockIdReader, DBProvider, DatabaseProviderFactory, HeaderProvider, StageCheckpointReader,
};
use reth_stages::StageId;
use reth_trie_pbt::TreeHasher;
use std::time::Instant;

pub(super) fn validate_startup<P>(strategy: &PbtStateRootStrategy, provider: &P) -> eyre::Result<()>
where
    P: DatabaseProviderFactory,
    P::Provider: HeaderProvider + StageCheckpointReader,
{
    let database = provider.database_provider_ro()?;
    let Some(retired) = database.mpt_retired_at()? else { return Ok(()) };
    let header = database
        .sealed_header(retired)?
        .ok_or_else(|| eyre::eyre!("missing MPT retirement header; re-anchor or resync"))?;
    strategy.validate_retired_parent(header.header(), Some(retired))?;
    eyre::ensure!(
        database
            .tx_ref()
            .get::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock)?
            .is_some_and(|finalized| finalized >= retired),
        "persisted finality is below MPT retirement; re-anchor or resync"
    );
    let checkpoint = database
        .get_stage_checkpoint(StageId::Finish)?
        .ok_or_else(|| eyre::eyre!("missing persisted state after MPT retirement"))?;
    eyre::ensure!(
        persisted_state(&checkpoint) >= retired,
        "persisted state is below MPT retirement; re-anchor or resync"
    );
    authenticate(strategy, &header)?;
    Ok(())
}

pub(super) fn try_retire<P, H: TreeHasher>(
    strategy: &PbtStateRootStrategy<H>,
    provider: &P,
) -> eyre::Result<bool>
where
    P: DatabaseProviderFactory + BlockIdReader + HeaderProvider,
    P::Provider: HeaderProvider + StageCheckpointReader,
{
    // Use locally accepted finality, never elapsed time or an unvalidated header alone.
    let Some(hash) = provider.finalized_block_hash()? else { return Ok(false) };
    let finalized = provider
        .sealed_header_by_hash(hash)?
        .ok_or_else(|| eyre::eyre!("missing finalized header during MPT retirement"))?;
    if finalized.timestamp() < strategy.activation_timestamp() {
        return Ok(false)
    }
    let view = provider.database_provider_ro()?;
    if view.mpt_retired_at()?.is_some() {
        return Ok(true)
    }
    let Some(checkpoint) = view.get_stage_checkpoint(StageId::Finish)? else { return Ok(false) };
    if persisted_state(&checkpoint) < finalized.number() {
        return Ok(false)
    }
    let Some(canonical) = view.sealed_header(finalized.number())? else { return Ok(false) };
    eyre::ensure!(
        canonical.hash() == finalized.hash(),
        "MPT retirement finality is not canonical in persisted state"
    );
    let tip = view
        .sealed_header(persisted_state(&checkpoint))?
        .ok_or_else(|| eyre::eyre!("missing persisted state header during MPT retirement"))?;
    drop(view);
    let mut checkpoints = vec![(finalized.hash(), finalized.state_root())];
    if tip.hash() != finalized.hash() {
        checkpoints.push((tip.hash(), tip.state_root()));
    }
    let blocks = checkpoints.iter().map(|(hash, _)| *hash).collect::<Vec<_>>();
    // Pin native roots throughout verification and commit, but acquire the MDBX writer
    // only after authentication. Pruning cannot invalidate the verified checkpoints.
    strategy.store().with_verified_blocks(&blocks, |roots| {
        for ((_, expected), root) in checkpoints.iter().zip(roots) {
            eyre::ensure!(
                root == expected,
                "PBT retirement checkpoint does not match its canonical header"
            );
        }
        let database = provider.database_provider_rw()?;
        if database.mpt_retired_at()?.is_some() {
            return Ok(true)
        }
        let view = provider.database_provider_ro()?;
        let Some(current) = view.get_stage_checkpoint(StageId::Finish)? else { return Ok(false) };
        // Retain the authenticated canonical prefix if persistence advanced. A reorg
        // away from the verified tip requires new authentication on the next tick.
        let Some(current_finalized_hash) = provider.finalized_block_hash()? else { return Ok(false) };
        let current_finalized = provider.sealed_header_by_hash(current_finalized_hash)?
            .ok_or_else(|| eyre::eyre!("missing current finalized header during MPT retirement"))?;
        if persisted_state(&current) < tip.number()
            || current_finalized.number() < finalized.number()
            || view.sealed_header(finalized.number())?.as_ref().map(SealedHeader::hash) != Some(finalized.hash())
            || view.sealed_header(tip.number())?.as_ref().map(SealedHeader::hash) != Some(tip.hash())
        {
            return Ok(false)
        }
        let current_tip = view.sealed_header(persisted_state(&current))?
            .ok_or_else(|| eyre::eyre!("missing current persisted header during MPT retirement"))?;
        drop(view);

        let commit = || -> eyre::Result<bool> {
            // MDBX serializes this transaction with persistence. A crash either retains both
            // tables without a marker, or commits the marker with both tables empty. TrieWriter
            // discards updates from jobs that began before this transaction committed.
            database.tx_ref().clear::<tables::AccountsTrie>()?;
            database.tx_ref().clear::<tables::StoragesTrie>()?;
            database
                .tx_ref()
                .put::<tables::ChainState>(tables::ChainStateKey::PbtMptRetiredBlock, finalized.number())?;
            // Preserve the rollback floor even if the process dies before the engine's next
            // ordinary finality flush. Never weaken a newer finalized checkpoint.
            let persisted_finalized = database
                .tx_ref()
                .get::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock)?
                .unwrap_or_default();
            database.tx_ref().put::<tables::ChainState>(
                tables::ChainStateKey::LastFinalizedBlock,
                persisted_finalized.max(finalized.number()),
            )?;
            database.commit()?;
            tracing::info!(target: "pbt::retirement", finalized = finalized.number(), hash = ?finalized.hash(), "Retired MPT node storage after PBT finality");
            Ok(true)
        };
        if current_tip.hash() == tip.hash() {
            commit()
        } else {
            // Descendant checkpoints were published by local execution after the
            // verified prefix. Publication syncs every new path before its reference.
            // Check that durable reference against the new canonical header, while
            // preserving the MDBX -> native lock order used by staged writers.
            Ok(strategy.store().with_published_block(current_tip.hash(), |root| {
                eyre::ensure!(root == current_tip.state_root(), "PBT retirement checkpoint does not match its canonical header");
                commit()
            })?.unwrap_or(false))
        }
    })
}

fn authenticate<H: BlockHeader>(
    strategy: &PbtStateRootStrategy,
    header: &SealedHeader<H>,
) -> eyre::Result<()> {
    eyre::ensure!(
        strategy.store().verify_block(header.hash())? == header.state_root(),
        "PBT retirement checkpoint does not match its canonical header"
    );
    Ok(())
}

fn persisted_state(checkpoint: &reth_stages::StageCheckpoint) -> u64 {
    checkpoint
        .finish_stage_checkpoint()
        .and_then(|finish| finish.partial_state_trie())
        .unwrap_or(checkpoint.block_number)
}

// Bound deferred garbage in removed-root units without scanning the complete state
// at every finality update. Explicit store collection remains available to callers.
const COLLECTION_ROOT_INTERVAL: u64 = 256;
// Start reclaiming outstanding garbage while there is still room for GC's writes.
const COLLECTION_MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub(super) fn prune_finalized<P>(
    strategy: &PbtStateRootStrategy,
    provider: &P,
    previous: Option<B256>,
    history: u64,
) -> eyre::Result<Option<B256>>
where
    P: DatabaseProviderFactory + BlockIdReader + HeaderProvider,
    P::Provider: HeaderProvider + StageCheckpointReader,
{
    let available = reth_fs_util::available_space(strategy.store().path().join("nodes"))?;
    prune_finalized_with_headroom(strategy, provider, previous, history, available)
}

fn prune_finalized_with_headroom<P>(
    strategy: &PbtStateRootStrategy,
    provider: &P,
    previous: Option<B256>,
    history: u64,
    available: u64,
) -> eyre::Result<Option<B256>>
where
    P: DatabaseProviderFactory + BlockIdReader + HeaderProvider,
    P::Provider: HeaderProvider + StageCheckpointReader,
{
    let pruned = prune_references(strategy, provider, previous, history)?;
    // Recheck collection even when reference pruning found no new finalized hash.
    // A pressure pass still requires outstanding debt, so low space alone cannot
    // cause a full-state scan on every maintenance tick after reclamation succeeds.
    let low_disk_space = available < COLLECTION_MIN_FREE_BYTES;
    let minimum_removed_roots = if low_disk_space { 1 } else { COLLECTION_ROOT_INTERVAL };
    let collection_started = Instant::now();
    if strategy.store().collect_if_needed(minimum_removed_roots)? {
        tracing::info!(target: "pbt::persistence", phase = "collection", low_disk_space, available_bytes = available, elapsed_ms = collection_started.elapsed().as_secs_f64() * 1000.0, "Collected unreachable PBT nodes");
    }
    Ok(pruned)
}

fn prune_references<P>(
    strategy: &PbtStateRootStrategy,
    provider: &P,
    previous: Option<B256>,
    history: u64,
) -> eyre::Result<Option<B256>>
where
    P: DatabaseProviderFactory + BlockIdReader + HeaderProvider,
    P::Provider: HeaderProvider + StageCheckpointReader,
{
    let Some(hash) = provider.finalized_block_hash()? else { return Ok(previous) };
    if previous == Some(hash) {
        return Ok(previous)
    }
    let finalized = provider
        .sealed_header_by_hash(hash)?
        .ok_or_else(|| eyre::eyre!("missing finalized header during PBT checkpoint pruning"))?;
    let view = provider.database_provider_ro()?;
    let Some(checkpoint) = view.get_stage_checkpoint(StageId::Finish)? else { return Ok(previous) };
    if persisted_state(&checkpoint) < finalized.number() {
        return Ok(previous)
    }
    eyre::ensure!(
        view.sealed_header(finalized.number())?.as_ref().map(SealedHeader::hash) == Some(hash),
        "PBT checkpoint pruning finality is not canonical"
    );
    let retired = if let Some(number) = view.mpt_retired_at()? {
        Some(
            view.sealed_header(number)?
                .ok_or_else(|| {
                    eyre::eyre!("missing MPT retirement header during PBT checkpoint pruning")
                })?
                .hash(),
        )
    } else {
        None
    };
    drop(view);
    let root = strategy.store().checkpoint_root(hash)?;
    if finalized.timestamp() >= strategy.activation_timestamp() {
        eyre::ensure!(
            root == finalized.state_root(),
            "PBT finalized checkpoint reference does not match its header"
        );
    } else if strategy.store().block_number(hash)?.is_none() {
        // Legacy pre-fork references lack a publication checksum.
        let trusted =
            strategy.restore_parent(hash, finalized.state_root(), finalized.timestamp())?;
        eyre::ensure!(
            root == trusted.root(),
            "PBT finalized checkpoint reference does not match its header"
        );
    }
    let floor = finalized.number().saturating_sub(history);
    let mut obsolete = Vec::new();
    for block in strategy.store().block_hashes()? {
        if block == hash || Some(block) == retired {
            continue;
        }
        let header = provider.sealed_header_by_hash(block)?;
        let recorded = strategy.store().block_number(block)?;
        if let Some(header) = &header &&
            let Some(number) = recorded
        {
            eyre::ensure!(
                number == header.number(),
                "PBT checkpoint block number does not match its header"
            );
        }
        // Legacy unknown references have no authenticated height and are retained.
        // New fork checkpoints carry a checksummed height even after their headers
        // leave the engine's memory, so abandoned forks cannot accumulate forever.
        let number = header.as_ref().map(|header| header.number()).or(recorded);
        if number.is_some_and(|number| number != 0 && number < floor) {
            obsolete.push(block);
        }
    }
    let outcome = strategy.store().prune_finalized_with_orphans(hash, &obsolete)?;
    tracing::debug!(target: "pbt::persistence", finalized = finalized.number(), outcome = ?outcome, "Pruned obsolete PBT checkpoints");
    Ok(Some(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{Address, U256};
    use reth_db_api::transaction::DbTxMut;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{create_test_provider_factory, insert_headers},
    };
    use reth_trie_pbt::{
        migration::DecodeLimits, persistence::SnapshotStore, AccountData, Blake3Hasher, StateTree,
    };
    use rocksdb::DB;
    use std::sync::Arc;

    #[test]
    fn disk_pressure_collects_dense_garbage_without_new_finality() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SnapshotStore::<Blake3Hasher>::open_with_limits(
            directory.path(),
            DecodeLimits::default(),
            DecodeLimits::native(),
        )?;
        let mut retained = StateTree::new();
        retained.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() })?;
        let header =
            SealedHeader::seal_slow(Header { state_root: retained.root(), ..Default::default() });
        store.save_block_with_number(header.hash(), 0, &retained)?;
        let mut orphan = retained.clone();
        for index in 1..=4096u64 {
            let address = Address::from_word(B256::from(U256::from(index)));
            orphan.set_account(address, AccountData { nonce: 1, ..Default::default() })?;
            for slot in [0, 64, 256] {
                orphan.set_storage(address, U256::from(slot), B256::repeat_byte(1))?;
            }
        }
        store.save(&orphan)?;
        assert_eq!(store.prune_finalized_with_orphans(header.hash(), &[])?.state_files, 1);
        drop(store);
        let provider = create_test_provider_factory();
        insert_headers(&provider, std::slice::from_ref(&header));
        let database = provider.database_provider_rw()?;
        database
            .tx_ref()
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, 0)?;
        database.commit()?;
        let provider = BlockchainProvider::new(provider)?;
        assert_eq!(provider.finalized_block_hash()?, Some(header.hash()));
        let node_key = |root: B256| [b"n".as_slice(), root.as_slice()].concat();
        for (available, reclaimed) in
            [(COLLECTION_MIN_FREE_BYTES, false), (COLLECTION_MIN_FREE_BYTES - 1, true)]
        {
            let store = Arc::new(SnapshotStore::open_with_limits(
                directory.path(),
                DecodeLimits::default(),
                DecodeLimits::native(),
            )?);
            let strategy = PbtStateRootStrategy::new(store, 0);
            assert_eq!(
                prune_finalized_with_headroom(
                    &strategy,
                    &provider,
                    Some(header.hash()),
                    64,
                    available
                )?,
                Some(header.hash())
            );
            assert_eq!(strategy.store().verify_block(header.hash())?, retained.root());
            if reclaimed {
                assert!(!strategy.store().collect_if_needed(1)?);
            }
            drop(strategy);
            let raw = DB::open_default(directory.path().join("nodes"))?;
            assert_eq!(raw.get(node_key(orphan.root()))?.is_none(), reclaimed);
            assert!(raw.get(node_key(retained.root()))?.is_some());
        }
        Ok(())
    }
}
