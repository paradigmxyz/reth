//! Downloads the code an account range's contracts reference, committing every verified response.

use crate::{common::DownloadContext, SnapBytecodeStore, SnapSyncError, VerifiedRange};
use reth_db_api::transaction::DbTxMut;
use reth_downloaders::snap::{BytecodeDownloader, BytecodeOutcome};
use reth_eth_wire_types::snap::GetByteCodesMessage;
use reth_network_p2p::snap::client::SnapClient;
use reth_network_peers::PeerId;
use reth_storage_api::{
    DBProvider, DatabaseProviderFactory, MetadataProvider, MetadataWriter, StateWriter,
};
use reth_tasks::Runtime;
use std::fmt;

/// Default number of code hashes asked for per request.
pub const DEFAULT_CODE_HASHES: usize = 128;

/// Downloads the code an account range still needs, one request at a time.
///
/// Code is content addressed, so a hash is requested only while no blob is stored for it, however
/// many accounts reference it and whichever attempt fetched it.
pub struct BytecodeDownload<C, F> {
    context: DownloadContext<C, F>,
    // Code hashes asked for per request.
    max_hashes: usize,
}

impl<C, F> BytecodeDownload<C, F> {
    /// Creates a download that requests whatever code the store does not already hold.
    pub const fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self {
            context: DownloadContext::new(client, factory, runtime),
            max_hashes: DEFAULT_CODE_HASHES,
        }
    }

    /// Returns this download asking peers for at most `response_bytes` per response.
    pub const fn with_response_bytes(mut self, response_bytes: u64) -> Self {
        self.context.set_response_bytes(response_bytes);
        self
    }

    /// Returns this download asking for at most `max_hashes` code hashes per request, at least one.
    pub const fn with_max_hashes(mut self, max_hashes: usize) -> Self {
        self.max_hashes = if max_hashes == 0 { 1 } else { max_hashes };
        self
    }
}

impl<C, F> BytecodeDownload<C, F>
where
    C: SnapClient + Clone + Unpin,
    F: DatabaseProviderFactory + Clone + 'static,
    F::Provider: MetadataProvider,
    F::ProviderRW: MetadataProvider + MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
{
    /// Requests code `range` still needs and commits the verified response.
    ///
    /// [`BytecodeStep::Complete`] once every hash it references is stored, so the range can commit
    /// without supplying code. Whatever a peer leaves out stays missing for the next call.
    pub async fn next(&mut self, range: &VerifiedRange) -> Result<BytecodeStep, SnapSyncError> {
        let write = range.write();
        let referenced = range.range().code_hashes();
        if referenced.is_empty() {
            return Ok(BytecodeStep::Complete)
        }
        let limit = self.max_hashes;
        let missing = self
            .context
            .read(move |provider| provider.missing_code(write, &referenced, limit))
            .await?;
        if missing.is_empty() {
            return Ok(BytecodeStep::Complete)
        }

        let request = GetByteCodesMessage {
            request_id: self.context.next_request_id(),
            hashes: missing,
            response_bytes: self.context.response_bytes(),
        };
        let downloader = BytecodeDownloader::new(
            self.context.client().clone(),
            request,
            self.context.runtime().clone(),
        )
        .expect("missing code is never empty");
        let codes = match downloader.await? {
            BytecodeOutcome::Verified(verified) => verified.into_codes(),
            BytecodeOutcome::Unavailable { peer_id } => {
                return Ok(BytecodeStep::Unavailable { peer_id })
            }
        };

        let persisted =
            self.context.commit(move |provider| provider.commit_bytecodes(write, codes)).await?;
        Ok(BytecodeStep::Committed { persisted })
    }
}

impl<C, F> fmt::Debug for BytecodeDownload<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BytecodeDownload")
            .field("context", &self.context)
            .field("max_hashes", &self.max_hashes)
            .finish()
    }
}

/// What one request of a [`BytecodeDownload`] produced.
#[derive(Debug)]
pub enum BytecodeStep {
    /// A response was committed, storing this many code blobs.
    Committed {
        /// Blobs the response supplied, at most the hashes it was asked for.
        persisted: usize,
    },
    /// The peer held none of the requested code, so all of it stays missing.
    Unavailable {
        /// Peer that answered without the code, so the retry can go elsewhere.
        peer_id: PeerId,
    },
    /// Every hash the range's accounts reference is stored.
    Complete,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, byte_codes, generation, hashed_factory, insert_generation_headers, key,
            state_root, verified_range, ScriptedSnapClient,
        },
        SnapAccountStore, SnapAttemptStore,
    };
    use alloy_primitives::{keccak256, Bytes, B256};
    use reth_db_api::{tables, transaction::DbTx};
    use reth_eth_wire_types::snap::ByteCodesMessage;
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::WithPeerId;
    use reth_provider::{test_utils::MockNodeTypesWithDB, ProviderFactory};
    use reth_trie_common::TrieAccount;
    use std::sync::Arc;

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type Download = BytecodeDownload<Arc<ScriptedSnapClient>, Factory>;

    fn code(byte: u8) -> Bytes {
        Bytes::from(vec![byte; 4])
    }

    fn contract(nonce: u64, code: &Bytes) -> TrieAccount {
        let mut contract = account(nonce);
        contract.code_hash = keccak256(code);
        contract
    }

    // Two contracts sharing one blob, a plain account, and a contract with its own blob.
    fn accounts() -> Vec<(B256, TrieAccount)> {
        vec![
            (key(1), contract(1, &code(1))),
            (key(2), account(2)),
            (key(3), contract(3, &code(1))),
            (key(4), contract(4, &code(2))),
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
        (Arc::clone(&client), BytecodeDownload::new(client, factory, Runtime::test()))
    }

    async fn committed(download: &mut Download, range: &VerifiedRange) -> usize {
        match download.next(range).await.unwrap() {
            BytecodeStep::Committed { persisted } => persisted,
            step => panic!("expected a commit, got {step:?}"),
        }
    }

    fn stored(factory: &Factory, code: &Bytes) -> bool {
        let provider = factory.database_provider_ro().unwrap();
        provider.tx_ref().get::<tables::Bytecodes>(keccak256(code)).unwrap().is_some()
    }

    #[tokio::test]
    async fn shared_code_is_requested_once_and_persisted_for_every_account() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (client, mut download) =
            download([byte_codes(1, &[code(1), code(2)])], factory.clone());

        assert_eq!(committed(&mut download, &range).await, 2);

        assert!(matches!(download.next(&range).await.unwrap(), BytecodeStep::Complete));
        assert_eq!(*client.code_requests(), [vec![keccak256(code(1)), keccak256(code(2))]]);
        assert!(stored(&factory, &code(1)) && stored(&factory, &code(2)));

        // With its code stored, the range commits without supplying any.
        let provider = factory.database_provider_rw().unwrap();
        let coverage = provider
            .commit_account_range(range.write(), range.range(), Default::default(), Vec::new())
            .unwrap();
        provider.commit().unwrap();
        assert!(coverage.is_complete());
    }

    #[tokio::test]
    async fn code_already_stored_is_never_requested() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (_, mut first) = download([byte_codes(1, &[code(1), code(2)])], factory.clone());
        committed(&mut first, &range).await;
        drop(first);

        let (client, mut resumed) = download([], factory);

        assert!(matches!(resumed.next(&range).await.unwrap(), BytecodeStep::Complete));
        assert!(client.code_requests().is_empty());
    }

    #[tokio::test]
    async fn code_a_peer_does_not_have_stays_missing() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let peer = PeerId::random();
        let empty = ByteCodesMessage { request_id: 1, codes: Vec::new() };
        let responses = [
            Ok(WithPeerId::new(peer, SnapResponse::ByteCodes(empty))),
            // A peer that holds only the first blob answers with a subsequence.
            byte_codes(2, &[code(1)]),
            byte_codes(3, &[code(2)]),
        ];
        let (client, mut download) = download(responses, factory.clone());

        let step = download.next(&range).await.unwrap();

        assert!(matches!(step, BytecodeStep::Unavailable { peer_id } if peer_id == peer));
        assert!(!stored(&factory, &code(1)));
        assert_eq!(committed(&mut download, &range).await, 1);
        // The unanswered hash is asked for again, the stored one is not.
        assert_eq!(committed(&mut download, &range).await, 1);
        assert!(matches!(download.next(&range).await.unwrap(), BytecodeStep::Complete));
        assert_eq!(
            *client.code_requests(),
            [
                vec![keccak256(code(1)), keccak256(code(2))],
                vec![keccak256(code(1)), keccak256(code(2))],
                vec![keccak256(code(2))],
            ]
        );
    }

    #[tokio::test]
    async fn unresolved_code_prevents_the_range_from_committing() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (_, mut download) = download([byte_codes(1, &[code(1)])], factory.clone());
        committed(&mut download, &range).await;

        let provider = factory.database_provider_rw().unwrap();
        let refused = provider.commit_account_range(
            range.write(),
            range.range(),
            Default::default(),
            Vec::new(),
        );

        assert!(matches!(
            refused,
            Err(SnapSyncError::MissingCode { hash }) if hash == keccak256(code(2))
        ));
    }

    #[tokio::test]
    async fn requests_ask_for_at_most_the_configured_hashes() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let responses = [byte_codes(1, &[code(1)]), byte_codes(2, &[code(2)])];
        let (client, download) = download(responses, factory);
        let mut download = download.with_max_hashes(1);

        committed(&mut download, &range).await;
        committed(&mut download, &range).await;

        assert!(matches!(download.next(&range).await.unwrap(), BytecodeStep::Complete));
        assert_eq!(*client.code_requests(), [vec![keccak256(code(1))], vec![keccak256(code(2))]]);
    }

    #[tokio::test]
    async fn a_range_without_contracts_requests_no_code() {
        let accounts = vec![(key(1), account(1)), (key(2), account(2))];
        let (factory, range) = started(&accounts);
        let (client, mut download) = download([], factory);

        assert!(matches!(download.next(&range).await.unwrap(), BytecodeStep::Complete));
        assert!(client.code_requests().is_empty());
    }

    #[tokio::test]
    async fn code_fetched_before_the_pivot_moved_is_not_committed() {
        let accounts = accounts();
        let (factory, range) = started(&accounts);
        let (client, mut download) =
            download([byte_codes(1, &[code(1), code(2)])], factory.clone());
        let provider = factory.database_provider_rw().unwrap();
        provider.advance_snap_pivot(range.write(), generation(2, state_root(&accounts))).unwrap();
        provider.commit().unwrap();

        assert!(matches!(download.next(&range).await, Err(SnapSyncError::StaleWrite { .. })));
        assert!(client.code_requests().is_empty());
    }
}
