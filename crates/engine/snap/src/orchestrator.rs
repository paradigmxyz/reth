//! Snap sync orchestrator — the main async loop that drives snap sync from start to finish.

use crate::{
    bal::{bal_to_state_diff, merge_account_diff},
    download::{download_state, initial_chunk_cursors, DownloadStateOutcome},
    finalize::write_snap_stage_checkpoints,
    pivot::PivotTracker,
    storage::{
        clear_hashed_state, read_hashed_account, write_bytecodes, write_hashed_accounts,
        write_hashed_storages,
    },
    SnapSyncError, SnapSyncEvent, SnapSyncOutcome, PIVOT_OFFSET,
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::B256;
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_primitives_traits::SealedHeader;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::{DBProvider, StateWriter, StorageSettingsCache};
use tokio::sync::mpsc::UnboundedReceiver;

/// Engine-driven snap sync orchestrator.
///
/// Runs as an async task spawned by the engine tree. Receives chain events
/// via an mpsc channel and drives the snap sync process through four phases:
///
/// 1. **Bootstrap** — wait for head, pick pivot, clear state
/// 2. **Bulk download** — download accounts, storage, bytecodes from peers
/// 3. **BAL catch-up** — apply remaining BAL diffs to reach latest known block
/// 4. **Verification** — compute and verify state root
#[derive(Debug)]
pub struct SnapSyncOrchestrator<C, F> {
    client: C,
    factory: F,
}

impl<C, F> SnapSyncOrchestrator<C, F>
where
    C: SnapClient + HeadersClient + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::Provider: DBProvider + HeaderProvider + StorageSettingsCache,
    F::ProviderRW: DBProvider + StateWriter + reth_provider::StaticFileProviderFactory,
    <F::Provider as DBProvider>::Tx: DbTx,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    /// Creates a new orchestrator with the given network client and database factory.
    pub fn new(client: C, factory: F) -> Self {
        Self { client, factory }
    }

    /// Runs the snap sync orchestrator to completion.
    ///
    /// This is the main entry point, intended to be spawned as a tokio task.
    /// `target_hash` is the FCU head block hash — used to resolve the head from
    /// peers when no `NewHead` event arrives (e.g. frozen-head / fresh-node scenario).
    pub async fn run(
        self,
        mut events_rx: UnboundedReceiver<SnapSyncEvent>,
        target_hash: B256,
    ) -> Result<SnapSyncOutcome, SnapSyncError> {
        // ── Phase 0: Bootstrap — wait for head, pick pivot, clear state ──────

        tracing::info!(target: "engine::snap_sync", %target_hash, "Starting snap sync orchestrator");

        let mut pre_buffered_blocks = Vec::new();
        let (initial_head_number, initial_head_hash) = loop {
            // Try to receive a NewHead from the engine tree first (non-blocking drain)
            match events_rx.try_recv() {
                Ok(SnapSyncEvent::NewHead { head_hash }) => {
                    let header = self
                        .client
                        .get_header(BlockHashOrNumber::Hash(head_hash))
                        .await
                        .map_err(|e| SnapSyncError::Network(format!("header fetch failed: {e}")))?
                        .into_data()
                        .ok_or_else(|| {
                            SnapSyncError::Network(format!(
                                "peer returned empty response for header {head_hash}"
                            ))
                        })?;
                    break (header.number(), head_hash);
                }
                Ok(event @ SnapSyncEvent::NewBlock { .. }) |
                Ok(event @ SnapSyncEvent::DownloadedBlock { .. }) => {
                    pre_buffered_blocks.push(event);
                    continue;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return Err(SnapSyncError::ChannelClosed);
                }
            }

            // No NewHead available yet — resolve from peers using the target hash.
            tracing::info!(
                target: "engine::snap_sync",
                %target_hash,
                "No NewHead event, resolving target header from peers"
            );
            let header = self
                .client
                .get_header(BlockHashOrNumber::Hash(target_hash))
                .await
                .map_err(|e| SnapSyncError::Network(format!("header fetch failed: {e}")))?
                .into_data()
                .ok_or_else(|| {
                    SnapSyncError::Network(format!(
                        "peer returned empty response for header {target_hash}"
                    ))
                })?;
            break (header.number(), target_hash);
        };

        let pivot_block = initial_head_number.saturating_sub(PIVOT_OFFSET);
        let initial_pivot = pivot_block;

        let pivot_root = {
            let from_buffer = pre_buffered_blocks.iter().find_map(|e| match e {
                SnapSyncEvent::NewBlock { number, state_root, .. } |
                SnapSyncEvent::DownloadedBlock { number, state_root, .. }
                    if *number == pivot_block =>
                {
                    Some(*state_root)
                }
                _ => None,
            });

            match from_buffer {
                Some(root) => root,
                None => {
                    // Try local DB first, fall back to fetching from peers
                    let local = self
                        .factory
                        .database_provider_ro()
                        .ok()
                        .and_then(|p| p.header_by_number(pivot_block).ok().flatten());
                    match local {
                        Some(h) => h.state_root(),
                        None => {
                            let h = self
                                .client
                                .get_header(BlockHashOrNumber::Number(pivot_block))
                                .await
                                .map_err(|e| {
                                    SnapSyncError::Network(format!(
                                        "pivot header fetch failed: {e}"
                                    ))
                                })?
                                .into_data()
                                .ok_or(SnapSyncError::MissingHeader(pivot_block))?;
                            h.state_root()
                        }
                    }
                }
            }
        };

        tracing::info!(
            target: "engine::snap_sync",
            pivot_block,
            %pivot_root,
            head = initial_head_number,
            "Picked pivot"
        );

        let mut tracker = PivotTracker::new(pivot_block, pivot_root, events_rx);
        tracker.set_known_head(initial_head_number, initial_head_hash);

        for event in pre_buffered_blocks {
            tracker.buffer_event(event);
        }

        clear_hashed_state(&self.factory)?;

        tracing::info!(target: "engine::snap_sync", "Cleared hashed state tables");

        // ── Phase 1: Bulk state download ─────────────────────────────────────
        //
        // Stream accounts in hash order. If the serving peer returns empty
        // (root is stale because chain advanced), advance the pivot to get a
        // fresh root and resume from the same position.

        tracing::info!(target: "engine::snap_sync", %pivot_root, "Phase 1: bulk state download");

        let mut download_cursors = initial_chunk_cursors();
        loop {
            let root = tracker.pivot_root();
            match download_state(&self.client, &self.factory, root, &mut download_cursors).await? {
                DownloadStateOutcome::Done => break,
                DownloadStateOutcome::Stale => {
                    tracing::info!(
                        target: "engine::snap_sync",
                        %root,
                        cursors = ?download_cursors,
                        "Pivot root stale, re-resolving head from peers"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                    // Drain any new events that arrived while we were sleeping.
                    tracker.drain_events();

                    // Try to discover chain advancement by probing a header
                    // a few blocks ahead of what we know. The serving node
                    // may have advanced beyond our initial head.
                    let probe = tracker.known_head() + 10;
                    if let Ok(response) =
                        self.client.get_header(BlockHashOrNumber::Number(probe)).await
                    {
                        if let Some(header) = response.into_data() {
                            let hash = SealedHeader::seal_slow(header).hash();
                            tracker.set_known_head(probe, hash);
                        }
                    }

                    tracker.advance_pivot(&self.client, &self.factory).await?;
                }
            }
        }

        tracing::info!(target: "engine::snap_sync", "Phase 1 complete: bulk download finished");

        // ── Phase 2: BAL catch-up ────────────────────────────────────────────

        tracing::info!(target: "engine::snap_sync", "Phase 2: BAL catch-up");

        tracker.drain_events();
        let final_block = tracker.known_head();

        if final_block > initial_pivot {
            for block_num in (initial_pivot + 1)..=final_block {
                let bal = tracker.get_verified_bal(&self.client, &self.factory, block_num).await?;

                let diff = bal_to_state_diff(&bal.account_changes);

                let mut merged = Vec::with_capacity(diff.accounts.len());
                for acct_diff in &diff.accounts {
                    let existing = read_hashed_account(&self.factory, acct_diff.hashed_address)?;
                    let account = merge_account_diff(acct_diff, existing.as_ref());
                    merged.push((acct_diff.hashed_address, account));
                }
                write_hashed_accounts(&self.factory, &merged)?;
                write_hashed_storages(&self.factory, &diff.storage)?;
                write_bytecodes(&self.factory, &diff.bytecodes)?;

                tracing::info!(
                    target: "engine::snap_sync",
                    block = block_num,
                    bal_bytes_len = bal.bytes.len(),
                    account_changes_count = bal.account_changes.len(),
                    diff_accounts = diff.accounts.len(),
                    diff_storage = diff.storage.len(),
                    diff_bytecodes = diff.bytecodes.len(),
                    "Applied BAL catch-up diff"
                );
            }
        }

        tracing::info!(
            target: "engine::snap_sync",
            final_block,
            "Phase 2 complete: BAL catch-up finished"
        );

        write_snap_stage_checkpoints(&self.factory, final_block)?;

        tracing::info!(
            target: "engine::snap_sync",
            block = final_block,
            "Snap sync complete — MerkleExecute stage will verify state root"
        );

        Ok(SnapSyncOutcome { synced_to: final_block, block_hash: tracker.known_head_hash() })
    }
}
