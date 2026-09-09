//! Downloads account ranges in key order and commits each with its dependencies.
//!
//! Every request captures the attempt's write before it is sent, and the commit presents that
//! write again. A response that arrives after the attempt was replaced or re-anchored is refused
//! before it changes state, and the coverage stays where it was.

use crate::{AccountCoverage, SnapAccountStore, SnapSyncError, SnapWrite};
use alloy_primitives::{map::B256Map, B256};
use reth_downloaders::snap::{AccountRangeDownloader, AccountRangeOutcome, VerifiedAccountRange};
use reth_eth_wire_types::snap::GetAccountRangeMessage;
use reth_network_p2p::snap::client::SnapClient;
use reth_storage_api::{DBProvider, DatabaseProviderFactory, MetadataProvider, SnapAttempt};
use reth_storage_errors::provider::ProviderError;
use reth_tasks::Runtime;
use reth_trie_common::HashedStorage;
use revm::bytecode::Bytecode;
use std::fmt;

// Matches the soft response limit peers commonly serve.
const DEFAULT_RESPONSE_BYTES: u64 = 512 * 1024;

// Keeps account requests inclusive through the full trie keyspace.
const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

/// Downloads the account ranges an attempt still needs, one at a time in key order.
///
/// [`Self::next`] fetches the range at the coverage cursor; the caller resolves its storage and
/// code, then [`Self::commit`] persists everything and moves the cursor.
pub struct AccountRangeDownload<C, F> {
    client: C,
    factory: F,
    // Proof verification and commits run on the blocking pool.
    runtime: Runtime,
    coverage: AccountCoverage,
    response_bytes: u64,
    // Distinguishes responses to reissued requests.
    request_id: u64,
}

impl<C, F> AccountRangeDownload<C, F> {
    /// Creates a download continuing from `coverage`.
    pub const fn new(client: C, factory: F, runtime: Runtime, coverage: AccountCoverage) -> Self {
        Self {
            client,
            factory,
            runtime,
            coverage,
            response_bytes: DEFAULT_RESPONSE_BYTES,
            request_id: 0,
        }
    }

    /// Returns this download asking peers for at most `response_bytes` per response.
    pub const fn with_response_bytes(mut self, response_bytes: u64) -> Self {
        self.response_bytes = response_bytes;
        self
    }

    /// How far the download has got.
    pub const fn coverage(&self) -> AccountCoverage {
        self.coverage
    }
}

impl<C, F> AccountRangeDownload<C, F>
where
    C: SnapClient + Clone + Unpin,
    F: DatabaseProviderFactory + Clone + 'static,
    F::Provider: MetadataProvider,
    F::ProviderRW: SnapAccountStore + DBProvider,
{
    /// Fetches the range at the coverage cursor.
    ///
    /// `Ok(None)` once every account is downloaded. A failed request leaves the cursor where it
    /// was, so the download can be resumed from the persisted coverage.
    pub async fn next(&mut self) -> Result<Option<AccountRangeStep>, SnapSyncError> {
        let Some(origin) = self.coverage.next() else { return Ok(None) };
        let (write, root_hash) = self.active_write()?;

        self.request_id = self.request_id.wrapping_add(1);
        let request = GetAccountRangeMessage {
            request_id: self.request_id,
            root_hash,
            starting_hash: origin,
            limit_hash: MAX_HASH,
            response_bytes: self.response_bytes,
        };
        let downloader =
            AccountRangeDownloader::new(self.client.clone(), request, self.runtime.clone())
                .expect("origin never exceeds the maximum hash");

        Ok(Some(match downloader.await? {
            AccountRangeOutcome::Verified(range) => {
                AccountRangeStep::Verified(VerifiedRange { origin, write, range })
            }
            AccountRangeOutcome::Unavailable { .. } => AccountRangeStep::Unavailable { origin },
        }))
    }

    /// Commits `verified` with the storage and code resolved for it, moving the cursor past it.
    ///
    /// Refused, with the coverage unchanged, when the attempt no longer accepts the write the
    /// range was fetched under or a dependency is missing.
    pub async fn commit(
        &mut self,
        verified: VerifiedRange,
        storages: B256Map<HashedStorage>,
        bytecodes: Vec<(B256, Bytecode)>,
    ) -> Result<AccountCoverage, SnapSyncError> {
        let factory = self.factory.clone();
        let coverage = self
            .runtime
            .spawn_blocking(move || -> Result<AccountCoverage, SnapSyncError> {
                let VerifiedRange { origin, write, range } = verified;
                let provider = factory.database_provider_rw()?;
                let coverage =
                    provider.commit_account_range(write, origin, &range, storages, bytecodes)?;
                provider.commit()?;
                Ok(coverage)
            })
            .await
            .map_err(|error| SnapSyncError::Provider(ProviderError::other(error)))??;
        self.coverage = coverage;
        Ok(coverage)
    }

    // The write the attempt accepts right now, and the root to request against.
    fn active_write(&self) -> Result<(SnapWrite, B256), SnapSyncError> {
        let provider = self.factory.database_provider_ro()?;
        let attempt = provider
            .snap_attempt()?
            .filter(SnapAttempt::is_unfinished)
            .ok_or(SnapSyncError::NoAttempt)?;
        Ok((SnapWrite::of(&attempt), attempt.state_root()))
    }
}

impl<C, F> fmt::Debug for AccountRangeDownload<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountRangeDownload")
            .field("coverage", &self.coverage)
            .field("response_bytes", &self.response_bytes)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

/// What one request of an [`AccountRangeDownload`] produced.
#[derive(Debug)]
pub enum AccountRangeStep {
    /// A range authenticated against the pivot root, waiting for its dependencies.
    Verified(VerifiedRange),
    /// No peer served the range, so the cursor stays at `origin`.
    Unavailable {
        /// Key the range was requested from.
        origin: B256,
    },
}

/// A verified range with the write it was fetched under.
///
/// Only [`AccountRangeDownload::commit`] consumes it, so the range is committed under the
/// attempt that was active when it was requested, never a later one.
#[derive(Debug)]
pub struct VerifiedRange {
    origin: B256,
    write: SnapWrite,
    range: VerifiedAccountRange,
}

impl VerifiedRange {
    /// Key the range was requested from.
    pub const fn origin(&self) -> B256 {
        self.origin
    }

    /// The accounts, whose storage and code the commit needs.
    pub const fn range(&self) -> &VerifiedAccountRange {
        &self.range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, account_range, generation, hashed_factory, key, state_root, ScriptedSnapClient,
        },
        SnapAttemptStore,
    };
    use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
    use reth_network_p2p::{
        error::{PeerRequestResult, RequestError},
        snap::client::SnapResponse,
    };
    use reth_provider::{test_utils::MockNodeTypesWithDB, ProviderFactory};
    use reth_trie_common::TrieAccount;
    use std::sync::Arc;

    const FAR: B256 = B256::repeat_byte(0xaa);

    fn accounts() -> Vec<(B256, TrieAccount)> {
        vec![(key(1), account(1)), (key(2), account(2)), (FAR, account(3))]
    }

    fn started(accounts: &[(B256, TrieAccount)]) -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider.commit().unwrap();
        factory
    }

    type Download =
        AccountRangeDownload<Arc<ScriptedSnapClient>, ProviderFactory<MockNodeTypesWithDB>>;

    fn download(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
        factory: ProviderFactory<MockNodeTypesWithDB>,
    ) -> (Arc<ScriptedSnapClient>, Download) {
        let client = Arc::new(ScriptedSnapClient::new(responses));
        let download = AccountRangeDownload::new(
            Arc::clone(&client),
            factory,
            Runtime::test(),
            AccountCoverage::START,
        );
        (client, download)
    }

    fn stored_accounts(factory: &ProviderFactory<MockNodeTypesWithDB>) -> Vec<B256> {
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        cursor.walk(None).unwrap().map(|entry| entry.unwrap().0).collect()
    }

    async fn verified(download: &mut Download) -> VerifiedRange {
        match download.next().await.unwrap().unwrap() {
            AccountRangeStep::Verified(verified) => verified,
            AccountRangeStep::Unavailable { .. } => panic!("fixture serves the range"),
        }
    }

    #[tokio::test]
    async fn downloads_the_trie_in_key_order_and_stops() {
        let accounts = accounts();
        let factory = started(&accounts);
        let responses = [
            // The first account only; the proof shows key 2 follows.
            account_range(1, &accounts, 0..1, &[key(1)]),
            // Everything from key 2 on.
            account_range(2, &accounts, 1..3, &[key(2), FAR]),
        ];
        let (client, mut download) = download(responses, factory.clone());

        let first = verified(&mut download).await;
        assert_eq!(first.origin(), B256::ZERO);
        assert_eq!(first.range().accounts().len(), 1);
        let coverage = download.commit(first, Default::default(), Vec::new()).await.unwrap();
        assert_eq!(coverage.next(), Some(key(2)));

        let second = verified(&mut download).await;
        assert_eq!(second.origin(), key(2));
        let coverage = download.commit(second, Default::default(), Vec::new()).await.unwrap();
        assert!(coverage.is_complete());

        assert!(download.next().await.unwrap().is_none());
        assert_eq!(*client.origins(), [B256::ZERO, key(2)]);
        assert_eq!(stored_accounts(&factory), [key(1), key(2), FAR]);
    }

    #[tokio::test]
    async fn an_unavailable_response_leaves_the_cursor_in_place() {
        let accounts = accounts();
        let factory = started(&accounts);
        let responses = [account_range(1, &[], 0..0, &[]), account_range(2, &accounts, 0..3, &[])];
        let (client, mut download) = download(responses, factory.clone());

        let step = download.next().await.unwrap().unwrap();

        assert!(matches!(step, AccountRangeStep::Unavailable { origin } if origin == B256::ZERO));
        assert_eq!(download.coverage(), AccountCoverage::START);

        let range = verified(&mut download).await;
        download.commit(range, Default::default(), Vec::new()).await.unwrap();
        assert!(download.coverage().is_complete());
        assert_eq!(*client.origins(), [B256::ZERO, B256::ZERO]);
    }

    #[tokio::test]
    async fn a_failed_request_leaves_the_cursor_in_place() {
        let factory = started(&accounts());
        let (_, mut download) =
            download([Err(RequestError::UnsupportedCapability)], factory.clone());

        let error = download.next().await.unwrap_err();

        assert!(matches!(error, SnapSyncError::Request(RequestError::UnsupportedCapability)));
        assert_eq!(download.coverage(), AccountCoverage::START);
        assert!(stored_accounts(&factory).is_empty());
    }

    #[tokio::test]
    async fn a_range_fetched_under_a_replaced_attempt_is_refused_at_commit() {
        let accounts = accounts();
        let factory = started(&accounts);
        let (_, mut download) = download([account_range(1, &accounts, 0..3, &[])], factory.clone());
        let range = verified(&mut download).await;
        // The attempt is replaced after the range was fetched.
        let provider = factory.database_provider_rw().unwrap();
        provider.start_snap_attempt(generation(1, state_root(&accounts))).unwrap();
        provider.commit().unwrap();

        let error = download.commit(range, Default::default(), Vec::new()).await.unwrap_err();

        assert!(matches!(error, SnapSyncError::StaleWrite { .. }));
        assert!(stored_accounts(&factory).is_empty());
        assert_eq!(download.coverage(), AccountCoverage::START);
    }

    #[tokio::test]
    async fn a_range_missing_its_dependencies_is_refused_at_commit() {
        let mut accounts = accounts();
        accounts[1].1.code_hash = B256::repeat_byte(0x33);
        let factory = started(&accounts);
        let (_, mut download) = download([account_range(1, &accounts, 0..3, &[])], factory.clone());
        let range = verified(&mut download).await;

        let error = download.commit(range, Default::default(), Vec::new()).await.unwrap_err();

        assert!(matches!(error, SnapSyncError::MissingCode { .. }));
        assert!(stored_accounts(&factory).is_empty());
        assert_eq!(download.coverage(), AccountCoverage::START);
    }

    #[tokio::test]
    async fn nothing_is_requested_without_an_active_attempt() {
        let (client, mut download) = download([], hashed_factory());

        assert!(matches!(download.next().await, Err(SnapSyncError::NoAttempt)));
        assert!(client.origins().is_empty());
    }

    #[tokio::test]
    async fn a_complete_coverage_requests_nothing() {
        let (client, mut download) = download([], hashed_factory());
        download.coverage = AccountCoverage::COMPLETE;

        assert!(download.next().await.unwrap().is_none());
        assert!(client.origins().is_empty());
    }
}
