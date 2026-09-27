//! Downloads the storage of an account range's contracts, committing every verified response.

use crate::{
    common::DownloadContext, SnapStorageStore, SnapSyncError, StorageChunk, StorageProgress,
    VerifiedRange, MAX_HASH,
};
use alloy_primitives::B256;
use reth_db_api::transaction::DbTxMut;
use reth_downloaders::snap::{
    StorageRangeDownloader, StorageRangeOutcome, VerifiedAccountBatch, VerifiedStorageRanges,
};
use reth_eth_wire_types::snap::GetStorageRangesMessage;
use reth_network_p2p::snap::client::SnapClient;
use reth_network_peers::PeerId;
use reth_storage_api::{
    DBProvider, DatabaseProviderFactory, MetadataProvider, MetadataWriter, StateWriter,
};
use reth_tasks::Runtime;
use std::fmt;

/// Default number of contracts asked for per storage request.
pub const DEFAULT_STORAGE_ACCOUNTS: usize = 128;

/// Downloads the storage an account range's contracts still need, one request at a time.
///
/// Every verified response is committed before the next request, so at most one response is held
/// however large a contract is, and a download resumes from the persisted progress.
pub struct StorageRangeDownload<C, F> {
    context: DownloadContext<C, F>,
    // Contracts asked for per request.
    max_accounts: usize,
}

impl<C, F> StorageRangeDownload<C, F> {
    /// Creates a download that continues from the progress the store records.
    pub const fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self {
            context: DownloadContext::new(client, factory, runtime),
            max_accounts: DEFAULT_STORAGE_ACCOUNTS,
        }
    }

    /// Returns this download asking peers for at most `response_bytes` per response.
    pub const fn with_response_bytes(mut self, response_bytes: u64) -> Self {
        self.context.set_response_bytes(response_bytes);
        self
    }

    /// Returns this download asking for at most `max_accounts` contracts per request, at least one.
    pub const fn with_max_accounts(mut self, max_accounts: usize) -> Self {
        self.max_accounts = if max_accounts == 0 { 1 } else { max_accounts };
        self
    }
}

impl<C, F> StorageRangeDownload<C, F>
where
    C: SnapClient + Clone + Unpin,
    F: DatabaseProviderFactory + Clone + 'static,
    F::Provider: MetadataProvider,
    F::ProviderRW: MetadataProvider + MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
{
    /// Requests storage `range` still needs and commits the verified response.
    ///
    /// [`StorageRangeStep::Complete`] once every contract in the range is persisted, so the range
    /// can commit without supplying their storage. A failed request leaves the progress in place.
    pub async fn next(&mut self, range: &VerifiedRange) -> Result<StorageRangeStep, SnapSyncError> {
        let (write, origin) = (range.write(), range.origin());
        let progress =
            self.context.factory().database_provider_ro()?.storage_progress(write, origin)?;
        let contracts = range.range().storage_batch();
        let Some(first) =
            contracts.accounts().iter().position(|(account, _)| !progress.is_complete(*account))
        else {
            return Ok(StorageRangeStep::Complete)
        };
        let end = progress.request_end(contracts.accounts(), first, self.max_accounts);
        let batch = contracts.range(first..end).expect("positions are inside the batch");
        let from = progress.resume_at(batch.accounts()[0].0).expect("first contract is incomplete");

        let request = GetStorageRangesMessage {
            request_id: self.context.next_request_id(),
            root_hash: batch.state_root(),
            account_hashes: batch.accounts().iter().map(|(account, _)| *account).collect(),
            starting_hash: from.into(),
            limit_hash: MAX_HASH.into(),
            response_bytes: self.context.response_bytes(),
        };
        let downloader = StorageRangeDownloader::new(
            self.context.client().clone(),
            request,
            &batch,
            self.context.runtime().clone(),
        )?;
        let ranges = match downloader.await? {
            StorageRangeOutcome::Verified(ranges) => ranges,
            StorageRangeOutcome::Unavailable { peer_id } => {
                return Ok(StorageRangeStep::Unavailable { peer_id })
            }
        };
        let chunks = chunks(ranges, batch, from)?;
        // One response's chunks commit together.
        let committed = self
            .context
            .commit(move |provider| {
                let mut progress = StorageProgress::START;
                for chunk in chunks {
                    progress = provider.commit_storage_chunk(write, origin, chunk)?;
                }
                Ok(progress)
            })
            .await?;
        Ok(StorageRangeStep::Committed(committed))
    }
}

impl<C, F> fmt::Debug for StorageRangeDownload<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageRangeDownload")
            .field("context", &self.context)
            .field("max_accounts", &self.max_accounts)
            .finish()
    }
}

/// What one request of a [`StorageRangeDownload`] produced.
#[derive(Debug)]
pub enum StorageRangeStep {
    /// A response was committed, reaching this progress.
    Committed(StorageProgress),
    /// The peer did not serve the storage, so the progress stays where it was.
    Unavailable {
        /// Peer that answered without the state, so the retry can go elsewhere.
        peer_id: PeerId,
    },
    /// Every contract in the range has its storage persisted.
    Complete,
}

// Splits a response into per-contract chunks. Only the last contract returned can be part way
// through, and it continues where a follow-up request would resume.
fn chunks(
    ranges: VerifiedStorageRanges,
    batch: VerifiedAccountBatch<'_>,
    from: B256,
) -> Result<Vec<StorageChunk>, SnapSyncError> {
    let roots: Vec<B256> =
        batch.accounts().iter().map(|(_, account)| account.storage_root).collect();
    let resume = ranges.follow_up(0, batch)?.map(|(request, _)| {
        (request.account_hashes[0], request.starting_hash.unwrap_or(B256::ZERO))
    });
    Ok(ranges
        .into_ranges()
        .into_iter()
        .zip(roots)
        .enumerate()
        .map(|(index, (range, storage_root))| {
            // Contracts after the first are requested whole.
            let from = if index == 0 { from } else { B256::ZERO };
            let next =
                resume.filter(|(account, _)| *account == range.account_hash).map(|(_, slot)| slot);
            StorageChunk::new(range.account_hash, storage_root, from, range.slots, next)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, generation, hashed_factory, insert_generation_headers, key, state_root,
            storage_ranges, storage_root_of, stored_slots, verified_range, ScriptedSnapClient,
        },
        SnapAccountStore, SnapAttemptStore, SnapCatchUpStore,
    };
    use alloy_eips::BlockNumHash;
    use alloy_primitives::U256;
    use reth_eth_wire_types::snap::StorageRangesMessage;
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::WithPeerId;
    use reth_provider::{test_utils::MockNodeTypesWithDB, ProviderFactory};
    use reth_trie_common::TrieAccount;
    use std::sync::Arc;

    const FAR: B256 = B256::repeat_byte(0xaa);

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type Download = StorageRangeDownload<Arc<ScriptedSnapClient>, Factory>;

    // Storage served over several responses.
    fn large() -> Vec<(B256, U256)> {
        vec![(key(1), U256::from(11)), (key(2), U256::from(12)), (key(3), U256::from(13))]
    }

    fn small() -> Vec<(B256, U256)> {
        vec![(key(9), U256::from(19))]
    }

    fn contract(nonce: u64, slots: &[(B256, U256)]) -> TrieAccount {
        let mut contract = account(nonce);
        contract.storage_root = storage_root_of(slots);
        contract
    }

    // A plain account, a large and a small contract, and a far plain account.
    fn accounts() -> Vec<(B256, TrieAccount)> {
        vec![
            (key(1), account(1)),
            (key(2), contract(2, &large())),
            (key(3), contract(3, &small())),
            (FAR, account(4)),
        ]
    }

    // An attempt that fetched all of `accounts` as one range, not yet committed.
    fn started(accounts: &[(B256, TrieAccount)]) -> (Factory, VerifiedRange) {
        let factory = hashed_factory();
        insert_generation_headers(&factory);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider.commit().unwrap();
        let range = verified_range(accounts, 0..accounts.len(), B256::ZERO, &[]);
        (factory, VerifiedRange::new(write, range))
    }

    fn download(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
        factory: Factory,
    ) -> (Arc<ScriptedSnapClient>, Download) {
        let client = Arc::new(ScriptedSnapClient::new(responses));
        (Arc::clone(&client), StorageRangeDownload::new(client, factory, Runtime::test()))
    }

    async fn committed(download: &mut Download, range: &VerifiedRange) -> StorageProgress {
        match download.next(range).await.unwrap() {
            StorageRangeStep::Committed(progress) => progress,
            step => panic!("expected a commit, got {step:?}"),
        }
    }

    fn slots_of(factory: &Factory, account: B256) -> Vec<(B256, U256)> {
        stored_slots(&factory.database_provider_ro().unwrap(), account)
    }

    #[tokio::test]
    async fn a_contract_spanning_responses_is_committed_response_by_response() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (large, small) = (large(), small());
        let responses = [
            // The large contract's first slot; the proof shows more follow.
            storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]),
            // The rest of it, without the small contract.
            storage_ranges(2, &[&large[1..]], &large, &[key(2), key(3)]),
            storage_ranges(3, &[&small[..]], &small, &[]),
        ];
        let (client, mut download) = download(responses, factory.clone());

        let progress = committed(&mut download, &range).await;
        assert_eq!(progress.resume_at(key(2)), Some(key(2)));
        assert_eq!(slots_of(&factory, key(2)), large[..1]);

        assert!(committed(&mut download, &range).await.is_complete(key(2)));
        assert!(committed(&mut download, &range).await.is_complete(key(3)));
        assert!(matches!(download.next(&range).await.unwrap(), StorageRangeStep::Complete));
        assert_eq!(
            *client.storage_requests(),
            [
                (vec![key(2), key(3)], B256::ZERO),
                (vec![key(2), key(3)], key(2)),
                (vec![key(3)], B256::ZERO),
            ]
        );

        // The range commits against the persisted storage, none of it supplied in memory.
        let provider = factory.database_provider_rw().unwrap();
        let coverage = provider
            .commit_account_range(range.write(), range.range(), Default::default(), Vec::new())
            .unwrap();
        provider.commit().unwrap();
        assert!(coverage.is_complete());
        assert_eq!(slots_of(&factory, key(2)), large);
        assert_eq!(slots_of(&factory, key(3)), small);
    }

    #[tokio::test]
    async fn an_unavailable_response_leaves_the_progress_in_place() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let large = large();
        let peer = PeerId::random();
        let empty = StorageRangesMessage { request_id: 2, slots: Vec::new(), proof: Vec::new() };
        let responses = [
            storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]),
            Ok(WithPeerId::new(peer, SnapResponse::StorageRanges(empty))),
            storage_ranges(3, &[&large[1..]], &large, &[key(2), key(3)]),
        ];
        let (client, mut download) = download(responses, factory.clone());
        let progress = committed(&mut download, &range).await;

        let step = download.next(&range).await.unwrap();

        assert!(matches!(step, StorageRangeStep::Unavailable { peer_id } if peer_id == peer));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.storage_progress(range.write(), range.origin()).unwrap(), progress);
        drop(provider);
        assert!(committed(&mut download, &range).await.is_complete(key(2)));
        let origins: Vec<_> = client.storage_requests().iter().map(|(_, from)| *from).collect();
        assert_eq!(origins, [B256::ZERO, key(2), key(2)]);
    }

    #[tokio::test]
    async fn a_new_download_resumes_from_the_last_committed_response() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (large, small) = (large(), small());
        let first = storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]);
        let (_, mut interrupted) = download([first], factory.clone());
        committed(&mut interrupted, &range).await;
        drop(interrupted);

        let responses = [
            storage_ranges(1, &[&large[1..]], &large, &[key(2), key(3)]),
            storage_ranges(2, &[&small[..]], &small, &[]),
        ];
        let (client, mut resumed) = download(responses, factory.clone());
        committed(&mut resumed, &range).await;
        committed(&mut resumed, &range).await;

        assert!(matches!(resumed.next(&range).await.unwrap(), StorageRangeStep::Complete));
        assert_eq!(
            *client.storage_requests(),
            [(vec![key(2), key(3)], key(2)), (vec![key(3)], B256::ZERO)]
        );
        assert_eq!(slots_of(&factory, key(2)), large);
    }

    #[tokio::test]
    async fn requests_ask_for_at_most_the_configured_contracts() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (large, small) = (large(), small());
        let responses = [
            storage_ranges(1, &[&large[..]], &large, &[]),
            storage_ranges(2, &[&small[..]], &small, &[]),
        ];
        let (client, download) = download(responses, factory);
        let mut download = download.with_max_accounts(1);

        committed(&mut download, &range).await;
        committed(&mut download, &range).await;

        assert!(matches!(download.next(&range).await.unwrap(), StorageRangeStep::Complete));
        assert_eq!(
            *client.storage_requests(),
            [(vec![key(2)], B256::ZERO), (vec![key(3)], B256::ZERO)]
        );
    }

    #[tokio::test]
    async fn a_range_without_contracts_requests_no_storage() {
        let accounts = vec![(key(1), account(1)), (FAR, account(2))];
        let (factory, range) = started(&accounts);
        let (client, mut download) = download([], factory);

        assert!(matches!(download.next(&range).await.unwrap(), StorageRangeStep::Complete));
        assert!(client.storage_requests().is_empty());
    }

    #[tokio::test]
    async fn storage_fetched_before_the_pivot_moved_is_not_resumed() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let large = large();
        let first = storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]);
        let (client, mut download) = download([first], factory.clone());
        committed(&mut download, &range).await;
        let provider = factory.database_provider_rw().unwrap();
        provider.advance_snap_pivot(range.write(), generation(2, state_root(&accounts))).unwrap();
        provider.commit().unwrap();

        assert!(matches!(download.next(&range).await, Err(SnapSyncError::StaleWrite { .. })));
        assert_eq!(client.storage_requests().len(), 1);
    }

    #[tokio::test]
    async fn storage_part_way_through_resumes_at_the_new_root() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (large, small) = (large(), small());
        // The new pivot turned the first account into a contract, ahead of the carried one.
        let mut moved = accounts.clone();
        moved[0].1 = contract(1, &small);
        let responses = [
            storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]),
            storage_ranges(2, &[&small[..]], &small, &[]),
            storage_ranges(3, &[&large[1..]], &large, &[key(2), key(3)]),
            storage_ranges(4, &[&small[..]], &small, &[]),
        ];
        let (client, mut download) = download(responses, factory.clone());
        committed(&mut download, &range).await;

        let provider = factory.database_provider_rw().unwrap();
        let write =
            provider.advance_snap_pivot(range.write(), generation(2, state_root(&moved))).unwrap();
        provider.commit().unwrap();
        let range =
            VerifiedRange::new(write, verified_range(&moved, 0..moved.len(), B256::ZERO, &[]));
        for _ in 0..3 {
            committed(&mut download, &range).await;
        }
        assert!(matches!(download.next(&range).await.unwrap(), StorageRangeStep::Complete));
        assert_eq!(
            *client.storage_requests(),
            [
                (vec![key(2), key(3)], B256::ZERO),
                (vec![key(1)], B256::ZERO),
                (vec![key(2), key(3)], key(2)),
                (vec![key(3)], B256::ZERO),
            ]
        );

        // The carried slot holds pivot 1's value until the lists reach the new pivot.
        let provider = factory.database_provider_rw().unwrap();
        assert!(matches!(
            provider.commit_account_range(write, range.range(), Default::default(), Vec::new()),
            Err(SnapSyncError::CatchUpBehindPivot { applied: 1, pivot: 2 })
        ));
        let block = BlockNumHash::new(2, B256::repeat_byte(2));
        provider.commit_block_access_list(write, block, B256::repeat_byte(1), &[]).unwrap();
        let coverage = provider
            .commit_account_range(write, range.range(), Default::default(), Vec::new())
            .unwrap();
        provider.commit().unwrap();
        assert!(coverage.is_complete());
        assert_eq!(slots_of(&factory, key(1)), small);
        assert_eq!(slots_of(&factory, key(2)), large);
    }

    #[tokio::test]
    async fn a_page_ending_before_a_carried_contract_hands_it_to_the_next_range() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let large = large();
        let first = storage_ranges(1, &[&large[..1]], &large, &[B256::ZERO, key(1)]);
        let (_, mut download) = download([first], factory.clone());
        committed(&mut download, &range).await;
        let provider = factory.database_provider_rw().unwrap();
        let write = provider
            .advance_snap_pivot(range.write(), generation(2, state_root(&accounts)))
            .unwrap();
        let block = BlockNumHash::new(2, B256::repeat_byte(2));
        provider.commit_block_access_list(write, block, B256::repeat_byte(1), &[]).unwrap();

        // The new root's page stops before the contract part way through.
        let page = verified_range(&accounts, 0..1, B256::ZERO, &[B256::ZERO, key(1)]);
        let next = provider
            .commit_account_range(write, &page, Default::default(), Vec::new())
            .unwrap()
            .next()
            .unwrap();

        assert!(next <= key(2));
        assert_eq!(provider.storage_progress(write, next).unwrap().resume_at(key(2)), Some(key(2)));
    }
}
