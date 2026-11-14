//! Reconstructs a finalized anchor's hashed-state view without mutating the live database.

use alloy_primitives::B256;
use reth_ethereum::{
    primitives::{AlloyBlockHeader, SealedHeader},
    provider::{
        BlockHashReader, BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
        DbTxProvider, HeaderProvider, LatestStateProviderRef, StageCheckpointReader,
        StorageChangeSetReader,
    },
    trie::{
        hashed_cursor::HashedPostStateCursorFactory, DatabaseHashedCursorFactory,
        DatabaseHashedPostState, HashedPostStateSorted,
    },
};
use reth_stages::StageId;
use reth_trie_pbt::persistence::SnapshotStore;
use std::path::Path;

pub(super) fn convert_anchor<P>(
    store: &SnapshotStore,
    provider: &P,
    anchor: &SealedHeader<P::Header>,
    preimages: &Path,
    max_blocks: usize,
    should_stop: impl FnMut() -> bool,
) -> eyre::Result<B256>
where
    P: DatabaseProviderFactory + HeaderProvider,
    P::Provider: BlockHashReader
        + BlockNumReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StageCheckpointReader,
{
    let database = provider.database_provider_ro()?.disable_long_read_transaction_safety();
    let accounts = database.get_stage_checkpoint(StageId::AccountHashing)?;
    let storage = database.get_stage_checkpoint(StageId::StorageHashing)?;
    eyre::ensure!(
        accounts.map(|checkpoint| checkpoint.block_number) ==
            storage.map(|checkpoint| checkpoint.block_number),
        "MPT conversion requires aligned account and storage hashing checkpoints"
    );
    let tip = match accounts {
        Some(checkpoint) => checkpoint.block_number,
        None => database.last_block_number()?,
    };
    let gap = tip
        .checked_sub(anchor.number())
        .ok_or_else(|| eyre::eyre!("MPT conversion anchor is beyond persisted state"))?;
    eyre::ensure!(gap <= u64::try_from(max_blocks)?, "MPT conversion history budget exceeded");
    let reverts = if gap == 0 {
        HashedPostStateSorted::default()
    } else {
        HashedPostStateSorted::from_reverts(&database, anchor.number() + 1..=tip)?
    };
    let cursors = HashedPostStateCursorFactory::new(
        DatabaseHashedCursorFactory::new(database.tx()),
        &reverts,
    );
    // Conversion runs before the engine starts; use the same pinned, timeout-free
    // view for bytecode rather than opening a second timed latest-state transaction.
    Ok(store.convert_anchor(
        anchor.hash(),
        anchor.state_root(),
        &cursors,
        &LatestStateProviderRef::new(&database),
        preimages,
        should_stop,
    )?)
}
