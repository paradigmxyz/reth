//! Pivot tracking, advancement, and BAL buffering.

use crate::{SnapSyncError, SnapSyncEvent, SNAP_RESPONSE_BYTES_LIMIT};
use alloy_consensus::BlockHeader;
use alloy_eip7928::{compute_block_access_list_hash, AccountChanges};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Decodable;
use reth_db_api::transaction::DbTx;
use reth_eth_wire_types::snap::GetBlockAccessListsMessage;
use reth_network_p2p::{
    headers::client::HeadersClient,
    snap::client::{SnapClient, SnapResponse},
};
use reth_primitives_traits::SealedHeader;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::DBProvider;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc::UnboundedReceiver;

/// When true, BALs served by peers for blocks whose header does not carry a
/// `block_access_list_hash` (i.e. pre-Amsterdam) are accepted without verification.
///
/// This exists so snap/2 can be tested against networks that do not propagate BALs (mainnet
/// today). It bypasses the normal integrity check; do not enable it outside of testing.
static TRUST_UNVERIFIED_BALS: AtomicBool = AtomicBool::new(false);

/// Toggle the global "trust unverified BALs" flag. See [`TRUST_UNVERIFIED_BALS`].
pub fn set_trust_unverified_bals(value: bool) {
    TRUST_UNVERIFIED_BALS.store(value, Ordering::Relaxed);
}

/// A block that has been received from the engine but not yet applied.
#[derive(Debug, Clone)]
struct BufferedBlock {
    /// State root from the block header.
    state_root: B256,
    /// RLP-encoded BAL bytes, if present.
    bal: Option<Bytes>,
}

/// A verified BAL and its decoded account changes.
pub(crate) struct VerifiedBal {
    /// Original RLP-encoded BAL bytes.
    pub bytes: Bytes,
    /// Decoded account changes.
    pub account_changes: Vec<AccountChanges>,
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
            SnapSyncEvent::NewBlock { number, state_root, bal, .. } => {
                self.buffered_blocks.insert(number, BufferedBlock { state_root, bal });
            }
            SnapSyncEvent::DownloadedBlock { number, state_root, .. } => {
                self.buffered_blocks.insert(number, BufferedBlock { state_root, bal: None });
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

    /// Gets and verifies a block BAL, checking the buffer first then fetching from peers.
    pub(crate) async fn get_verified_bal<C, F>(
        &self,
        client: &C,
        factory: &F,
        block_num: u64,
    ) -> Result<VerifiedBal, SnapSyncError>
    where
        C: SnapClient + HeadersClient + 'static,
        F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
        F::Provider: DBProvider + HeaderProvider,
        <F::Provider as DBProvider>::Tx: DbTx,
    {
        let (block_hash, expected_hash) =
            self.resolve_header_hash(client, factory, block_num).await?;

        let bal = if let Some(block) = self.buffered_blocks.get(&block_num) {
            block.bal.clone()
        } else {
            None
        };

        let bal = match bal {
            Some(bal) => bal,
            None => {
                let response = client
                    .get_snap_block_access_lists(GetBlockAccessListsMessage {
                        request_id: 0,
                        block_hashes: vec![block_hash],
                        response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
                    })
                    .await
                    .map_err(|e| {
                        SnapSyncError::Network(format!(
                            "snap/2 BAL fetch for block {block_num}: {e}"
                        ))
                    })?;
                let SnapResponse::BlockAccessLists(message) = response.into_data() else {
                    return Err(SnapSyncError::Network(format!(
                        "peer returned non-BAL snap response for block {block_num}"
                    )));
                };

                let bal = message
                    .block_access_lists
                    .0
                    .into_iter()
                    .next()
                    .ok_or(SnapSyncError::MissingBal(block_num))?;
                if bal.as_ref() == [alloy_rlp::EMPTY_STRING_CODE] {
                    return Err(SnapSyncError::MissingBal(block_num));
                }
                bal
            }
        };

        let account_changes: Vec<AccountChanges> = Vec::<AccountChanges>::decode(&mut bal.as_ref())
            .map_err(|e| {
                SnapSyncError::RlpDecode(format!("BAL decode at block {block_num}: {e}"))
            })?;
        let got = compute_block_access_list_hash(&account_changes);
        match expected_hash {
            Some(expected) => {
                if got != expected {
                    return Err(SnapSyncError::BalVerification { block: block_num, expected, got });
                }
            }
            None => {
                // The header carries no `block_access_list_hash`. This happens for pre-Amsterdam
                // blocks; the BAL was generated by the serving peer rather than committed by the
                // chain. Accept it only if the operator has explicitly opted in.
                if !TRUST_UNVERIFIED_BALS.load(Ordering::Relaxed) {
                    return Err(SnapSyncError::BalVerification {
                        block: block_num,
                        expected: B256::ZERO,
                        got,
                    });
                }
                tracing::debug!(
                    target: "engine::snap_sync",
                    block_num,
                    %got,
                    "Accepting unverified BAL (header has no block_access_list_hash)"
                );
            }
        }

        Ok(VerifiedBal { bytes: bal, account_changes })
    }

    async fn resolve_header_hash<C, F>(
        &self,
        client: &C,
        factory: &F,
        block_num: u64,
    ) -> Result<(B256, Option<B256>), SnapSyncError>
    where
        C: HeadersClient + 'static,
        F: DatabaseProviderFactory,
        F::Provider: DBProvider + HeaderProvider,
        <F::Provider as DBProvider>::Tx: DbTx,
    {
        let local = factory
            .database_provider_ro()
            .ok()
            .and_then(|p| p.header_by_number(block_num).ok().flatten());

        match local {
            Some(header) => {
                let expected = header.block_access_list_hash();
                Ok((SealedHeader::seal_slow(header).hash(), expected))
            }
            None => {
                let header = client
                    .get_header(BlockHashOrNumber::Number(block_num))
                    .await
                    .map_err(|e| {
                        SnapSyncError::Network(format!("header fetch for block {block_num}: {e}"))
                    })?
                    .into_data()
                    .ok_or(SnapSyncError::MissingHeader(block_num))?;
                let expected = header.block_access_list_hash();
                Ok((SealedHeader::seal_slow(header).hash(), expected))
            }
        }
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
