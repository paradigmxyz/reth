//! Downloads authenticated bytecode dependencies through all required responses.

use crate::{
    request::{push_peer, request_options, SnapRequests},
    SnapSyncError, DEFAULT_RESPONSE_BYTES,
};
use alloy_primitives::{map::B256Set, Bytes, B256, KECCAK256_EMPTY};
use reth_downloaders::snap::{BytecodeDownloader, BytecodeOutcome};
use reth_eth_wire_types::snap::GetByteCodesMessage;
use reth_network_p2p::{error::RequestError, snap::client::SnapClient};
use reth_trie_common::TrieAccount;

// Four average-sized requests per maximum EVM bytecode balances gaps against truncation.
const BYTECODE_HASHES_PER_REQUEST: usize = DEFAULT_RESPONSE_BYTES as usize / (24 * 1024) * 4;

impl<C: SnapClient> SnapRequests<'_, C> {
    // Preserves missing hashes across truncated responses until the batch is complete.
    pub(crate) async fn download_bytecodes(
        &mut self,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<Option<Vec<(B256, Bytes)>>, SnapSyncError> {
        let mut seen = B256Set::default();
        let hashes = accounts
            .iter()
            .map(|(_, account)| account.code_hash)
            .filter(|hash| *hash != KECCAK256_EMPTY && seen.insert(*hash))
            .collect::<Vec<_>>();
        let mut bytecodes = Vec::with_capacity(hashes.len());

        for chunk in hashes.chunks(BYTECODE_HASHES_PER_REQUEST) {
            let mut pending = chunk.to_vec();
            let mut excluded = Vec::new();
            while !pending.is_empty() {
                let request = GetByteCodesMessage {
                    request_id: self.next_id()?,
                    hashes: pending.clone(),
                    response_bytes: DEFAULT_RESPONSE_BYTES,
                };
                let downloader = BytecodeDownloader::new_with_options(
                    self.client,
                    request,
                    self.runtime.clone(),
                    request_options(&excluded),
                )
                .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;
                match downloader.await {
                    Ok(BytecodeOutcome::Unavailable { peer_id }) => {
                        pending = chunk
                            .iter()
                            .copied()
                            .filter(|hash| !bytecodes.iter().any(|(stored, _)| stored == hash))
                            .collect();
                        push_peer(&mut excluded, peer_id);
                    }
                    Err(RequestError::UnsupportedCapability) => return Ok(None),
                    Err(error) => return Err(error.into()),
                    Ok(BytecodeOutcome::Verified(verified)) => {
                        pending = verified.missing(&pending).collect();
                        bytecodes.extend(verified.into_codes());
                        excluded.clear();
                    }
                }
            }
        }
        Ok(Some(bytecodes))
    }
}
