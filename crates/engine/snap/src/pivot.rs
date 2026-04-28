//! Pivot tracking, advancement, and BAL buffering.

use crate::{SnapSyncError, SnapSyncEvent};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Bytes, B256};
use reth_db_api::transaction::DbTx;
use reth_network_p2p::{
    block_access_lists::client::BlockAccessListsClient, headers::client::HeadersClient,
};
use reth_primitives_traits::SealedHeader;
use reth_provider::{BlockHashReader, DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::DBProvider;
use std::collections::BTreeMap;
use tokio::sync::mpsc::UnboundedReceiver;

/// A block that has been received from the engine but not yet applied.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct BufferedBlock {
    /// Block hash.
    pub hash: B256,
    /// State root from the block header.
    pub state_root: B256,
    /// Parent hash.
    pub parent_hash: B256,
    /// RLP-encoded BAL bytes, if present.
    pub bal: Option<Bytes>,
}

/// Tracks the current pivot block and buffers incoming BALs from the engine.
#[derive(Debug)]
pub struct PivotTracker {
    /// Current pivot block number.
    pivot_block: u64,
    /// State root at the current pivot.
    pivot_root: B256,
    /// Known head block number (from NewHead events).
    known_head: u64,
    /// Known head hash.
    known_head_hash: B256,
    /// Buffered blocks received via `SnapSyncEvent::NewBlock`, keyed by block number.
    buffered_blocks: BTreeMap<u64, BufferedBlock>,
    /// Event receiver from the engine.
    events_rx: UnboundedReceiver<SnapSyncEvent>,
}

impl PivotTracker {
    /// Creates a new tracker with the given pivot and an empty buffer.
    pub fn new(
        pivot_block: u64,
        pivot_root: B256,
        events_rx: UnboundedReceiver<SnapSyncEvent>,
    ) -> Self {
        Self {
            pivot_block,
            pivot_root,
            known_head: 0,
            known_head_hash: B256::ZERO,
            buffered_blocks: BTreeMap::new(),
            events_rx,
        }
    }

    /// Returns the current pivot block number.
    pub fn pivot_block(&self) -> u64 {
        self.pivot_block
    }

    /// Returns the state root at the current pivot.
    pub fn pivot_root(&self) -> B256 {
        self.pivot_root
    }

    /// Returns the known head block number.
    pub fn known_head(&self) -> u64 {
        self.known_head
    }

    /// Returns the known head block hash.
    pub fn known_head_hash(&self) -> B256 {
        self.known_head_hash
    }

    /// Sets the known head block number and hash.
    pub fn set_known_head(&mut self, number: u64, hash: B256) {
        if number > self.known_head {
            self.known_head = number;
            self.known_head_hash = hash;
        }
    }

    /// Processes a single event, buffering it into the tracker's state.
    pub(crate) fn buffer_event(&mut self, event: SnapSyncEvent) {
        match event {
            SnapSyncEvent::NewBlock { number, hash, state_root, parent_hash, bal } => {
                self.buffered_blocks
                    .insert(number, BufferedBlock { hash, state_root, parent_hash, bal });
            }
            SnapSyncEvent::DownloadedBlock { number, hash, state_root, parent_hash } => {
                self.buffered_blocks
                    .insert(number, BufferedBlock { hash, state_root, parent_hash, bal: None });
            }
            SnapSyncEvent::NewHead { head_hash } => {
                // Hash-only: we don't update known_head number here.
                // The orchestrator resolves the number from peers at bootstrap.
                self.known_head_hash = head_hash;
            }
        }
    }

    /// Drains all pending events from the engine channel (non-blocking).
    pub fn drain_events(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.buffer_event(event);
        }
    }

    /// Advances the pivot to the latest known head without applying BAL diffs.
    ///
    /// This only bumps the pivot block/root so that subsequent download requests
    /// use a fresh root the serving peer can satisfy. BAL healing is done once
    /// after all downloading completes (Phase 2 in the orchestrator).
    ///
    /// Returns `Ok(true)` if the pivot was advanced, `Ok(false)` if already at head.
    pub async fn advance_pivot<C, F>(
        &mut self,
        client: &C,
        factory: &F,
    ) -> Result<bool, SnapSyncError>
    where
        C: HeadersClient + 'static,
        F: DatabaseProviderFactory,
        F::Provider: DBProvider + HeaderProvider,
        <F::Provider as DBProvider>::Tx: DbTx,
    {
        self.drain_events();

        let new_pivot = self.known_head.saturating_sub(crate::PIVOT_OFFSET);
        if new_pivot <= self.pivot_block {
            return Ok(false);
        }

        let old_pivot = self.pivot_block;
        let new_root = self.resolve_state_root(client, factory, new_pivot).await?;

        self.pivot_block = new_pivot;
        self.pivot_root = new_root;
        self.buffered_blocks = self.buffered_blocks.split_off(&(new_pivot.saturating_sub(10)));

        tracing::info!(target: "engine::snap_sync", old_pivot, new_pivot, %new_root, "Advanced pivot");

        Ok(true)
    }

    /// Gets BAL bytes for a block, checking the buffer first then fetching from peers.
    pub(crate) async fn get_bal_bytes<C, F>(
        &self,
        client: &C,
        factory: &F,
        block_num: u64,
    ) -> Result<Bytes, SnapSyncError>
    where
        C: BlockAccessListsClient + HeadersClient + 'static,
        F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
        F::Provider: DBProvider + HeaderProvider + BlockHashReader,
        <F::Provider as DBProvider>::Tx: DbTx,
    {
        if let Some(block) = self.buffered_blocks.get(&block_num) {
            if let Some(ref bal) = block.bal {
                return Ok(bal.clone());
            }
        }

        // Resolve block hash: try local DB, fall back to fetching header from peers
        let block_hash = {
            let local = factory
                .database_provider_ro()
                .ok()
                .and_then(|p| p.block_hash(block_num).ok().flatten());
            match local {
                Some(hash) => hash,
                None => {
                    let header = client
                        .get_header(BlockHashOrNumber::Number(block_num))
                        .await
                        .map_err(|e| {
                            SnapSyncError::Network(format!(
                                "header fetch for block {block_num}: {e}"
                            ))
                        })?
                        .into_data()
                        .ok_or(SnapSyncError::MissingBlockHash(block_num))?;
                    SealedHeader::seal_slow(header).hash()
                }
            }
        };

        let response = client
            .get_block_access_lists(vec![block_hash])
            .await
            .map_err(|e| SnapSyncError::Network(format!("BAL fetch for block {block_num}: {e}")))?;
        let bals = response.into_data();

        bals.0.into_iter().next().ok_or(SnapSyncError::MissingBal(block_num))
    }

    /// Resolves the state root for a block number from the buffer, local DB, or peers.
    async fn resolve_state_root<C, F>(
        &self,
        client: &C,
        factory: &F,
        block_num: u64,
    ) -> Result<B256, SnapSyncError>
    where
        C: HeadersClient + 'static,
        F: DatabaseProviderFactory,
        F::Provider: DBProvider + HeaderProvider,
        <F::Provider as DBProvider>::Tx: DbTx,
    {
        if let Some(block) = self.buffered_blocks.get(&block_num) {
            return Ok(block.state_root);
        }

        // Try local DB first, fall back to fetching header from peers
        let local = factory
            .database_provider_ro()
            .ok()
            .and_then(|p| p.header_by_number(block_num).ok().flatten());

        match local {
            Some(header) => Ok(header.state_root()),
            None => {
                let header = client
                    .get_header(BlockHashOrNumber::Number(block_num))
                    .await
                    .map_err(|e| {
                        SnapSyncError::Network(format!(
                            "pivot header fetch for block {block_num}: {e}"
                        ))
                    })?
                    .into_data()
                    .ok_or(SnapSyncError::MissingHeader(block_num))?;
                Ok(header.state_root())
            }
        }
    }
}
