//! Downloads authenticated storage dependencies through all required responses.

use crate::{
    common::{push_peer, request_options, SnapRequests},
    SnapSyncError, DEFAULT_RESPONSE_BYTES, MAX_HASH,
};
use alloy_primitives::B256;
use reth_downloaders::snap::{StorageRangeDownloader, StorageRangeOutcome, VerifiedAccountBatch};
use reth_eth_wire_types::snap::GetStorageRangesMessage;
use reth_network_p2p::{error::RequestError, snap::client::SnapClient};
use reth_trie_common::HashedStorage;

impl<C: SnapClient> SnapRequests<'_, C> {
    // Completes every non-empty storage trie before its owning accounts become durable.
    pub(crate) async fn download_storages(
        &mut self,
        batch: VerifiedAccountBatch<'_>,
    ) -> Result<Option<Vec<(B256, HashedStorage)>>, SnapSyncError> {
        if batch.accounts().is_empty() {
            return Ok(Some(Vec::new()))
        }

        let mut storages = batch
            .accounts()
            .iter()
            .map(|(hash, _)| (*hash, HashedStorage::default()))
            .collect::<Vec<_>>();
        if self.download_storage_batch(batch, &mut storages).await?.is_none() {
            return Ok(None)
        }
        Ok(Some(storages))
    }

    // Drives one contiguous non-empty account batch through all authenticated follow-ups.
    async fn download_storage_batch(
        &mut self,
        mut batch: VerifiedAccountBatch<'_>,
        storages: &mut [(B256, HashedStorage)],
    ) -> Result<Option<()>, SnapSyncError> {
        let mut request = GetStorageRangesMessage {
            request_id: self.next_id()?,
            root_hash: batch.state_root(),
            account_hashes: batch.accounts().iter().map(|(hash, _)| *hash).collect(),
            starting_hash: B256::ZERO.into(),
            limit_hash: MAX_HASH.into(),
            response_bytes: DEFAULT_RESPONSE_BYTES,
        };
        let mut excluded = Vec::new();
        loop {
            let downloader = StorageRangeDownloader::new_with_options(
                self.client,
                request.clone(),
                &batch,
                self.runtime.clone(),
                request_options(&excluded),
            )
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
            match downloader.await {
                Ok(StorageRangeOutcome::Unavailable { peer_id }) => {
                    push_peer(&mut excluded, peer_id)
                }
                Err(RequestError::UnsupportedCapability) => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(StorageRangeOutcome::Verified(verified)) => {
                    let follow_up_id = self.request_id.wrapping_add(1);
                    let follow_up = verified
                        .follow_up(follow_up_id, batch)
                        .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
                    for range in verified.into_ranges() {
                        let storage = storages
                            .iter_mut()
                            .find(|(hash, _)| *hash == range.account_hash)
                            .expect("verified range belongs to a requested account");
                        storage.1.storage.extend(range.slots);
                    }
                    let Some((next_request, next_batch)) = follow_up else { return Ok(Some(())) };
                    if follow_up_id == 0 {
                        return Err(SnapSyncError::InvalidRequest(
                            "snap request id space exhausted".to_string(),
                        ))
                    }
                    self.request_id = follow_up_id;
                    request = next_request;
                    batch = next_batch;
                    excluded.clear();
                }
            }
        }
    }
}
