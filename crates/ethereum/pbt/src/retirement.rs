//! Finality-gated MPT disposal, committed atomically with a permanent database marker.

use reth_engine_tree::tree::state_root_strategy::pbt::PbtStateRootStrategy;
use reth_ethereum::{
    primitives::{AlloyBlockHeader, BlockHeader, SealedHeader},
    provider::{
        db::{
            tables,
            transaction::{DbTx, DbTxMut},
        },
        BlockIdReader, DBProvider, DatabaseProviderFactory, HeaderProvider, StageCheckpointReader,
    },
};
use reth_stages::StageId;

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

pub(super) fn try_retire<P>(strategy: &PbtStateRootStrategy, provider: &P) -> eyre::Result<bool>
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
    let database = provider.database_provider_rw()?;
    if database.mpt_retired_at()?.is_some() {
        return Ok(true)
    }
    // Keep the write transaction while opening the read view: persistence and reorgs
    // cannot move the durable frontier between authentication and deletion.
    let view = provider.database_provider_ro()?;
    let Some(checkpoint) = view.get_stage_checkpoint(StageId::Finish)? else { return Ok(false) };
    // A deferred persistence frontier can trail Finish's ordinary block-data frontier.
    if persisted_state(&checkpoint) < finalized.number() {
        return Ok(false)
    }
    let Some(canonical) = view.sealed_header(finalized.number())? else { return Ok(false) };
    eyre::ensure!(
        canonical.hash() == finalized.hash(),
        "MPT retirement finality is not canonical in persisted state"
    );
    // Check both the synced native checkpoint and its block reference before losing MPT.
    authenticate(strategy, &finalized)?;
    let tip = view
        .sealed_header(persisted_state(&checkpoint))?
        .ok_or_else(|| eyre::eyre!("missing persisted state header during MPT retirement"))?;
    if tip.hash() != finalized.hash() {
        authenticate(strategy, &tip)?;
    }
    drop(view);

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
