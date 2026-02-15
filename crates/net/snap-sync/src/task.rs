//! Snap sync task for integration with the node builder.
//!
//! Provides a standalone async function that runs snap sync to completion,
//! suitable for spawning as a background task before the pipeline starts.

use crate::{downloader::SnapSyncDownloader, error::SnapSyncError};
use alloy_primitives::B256;
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_network_p2p::snap::client::SnapClient;
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use tracing::info;

/// Runs snap sync to completion, returning the pivot block number on success.
///
/// This is the main entry point for integrating snap sync with the node builder.
/// It creates a [`SnapSyncDownloader`], runs all phases (account download, storage
/// download, bytecode download, state root verification), and returns the pivot
/// block number that the pipeline should resume from.
pub async fn run_snap_sync<C, F>(
    client: C,
    provider_factory: F,
    pivot_hash: B256,
    pivot_number: u64,
    state_root: B256,
) -> Result<u64, SnapSyncError>
where
    C: SnapClient + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory + Send + Sync + 'static,
    <F as DatabaseProviderFactory>::ProviderRW: DBProvider<Tx: DbTxMut + DbTx> + Send,
{
    let (_, cancel_rx) = tokio::sync::watch::channel(false);
    let mut downloader = SnapSyncDownloader::new(
        client,
        provider_factory,
        pivot_hash,
        pivot_number,
        state_root,
        cancel_rx,
    );

    info!(target: "snap::sync", pivot_number, %pivot_hash, "Starting snap sync");
    downloader.run().await?;
    info!(target: "snap::sync", pivot_number, "Snap sync completed successfully");

    Ok(pivot_number)
}
