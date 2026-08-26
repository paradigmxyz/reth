//! Downloads and verifies snap/2 account ranges against
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189) pivot state roots.
//!
//! Persistence and range selection are handled by the snap sync orchestrator.

use alloy_primitives::B256;
use futures::{Future, FutureExt};
use reth_eth_wire_types::snap::{AccountRangeMessage, GetAccountRangeMessage};
use reth_network_p2p::{
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;
use reth_trie_common::{range_proof::verify_range_proof, TrieAccount, EMPTY_ROOT_HASH};
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

const MAX_RETRIES: u8 = 2;

/// Downloads and verifies one account range against its requested state root.
///
/// Invalid responses penalize their peer and retry at high priority. Proof verification runs on
/// the blocking pool.
pub struct AccountRangeDownloader<C: SnapClient> {
    client: C,
    runtime: Runtime,
    request: GetAccountRangeMessage,
    fut: C::Output,
    verification: Option<VerificationTask>,
    retries: u8,
}

impl<C: SnapClient> std::fmt::Debug for AccountRangeDownloader<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountRangeDownloader")
            .field("client", &self.client)
            .field("request", &self.request)
            .field("verification", &self.verification)
            .field("retries", &self.retries)
            .finish_non_exhaustive()
    }
}

impl<C: SnapClient> AccountRangeDownloader<C> {
    /// Creates a downloader using `runtime` for proof verification and submits the initial request.
    /// Returns an error when the origin exceeds the limit.
    pub fn new(
        client: C,
        request: GetAccountRangeMessage,
        runtime: Runtime,
    ) -> Result<Self, InvalidAccountRange> {
        if request.starting_hash > request.limit_hash {
            return Err(InvalidAccountRange {
                origin: request.starting_hash,
                limit: request.limit_hash,
            })
        }
        let fut = client.get_account_range(request.clone());
        Ok(Self { client, runtime, request, fut, verification: None, retries: 0 })
    }

    // Raise retry priority so transient failures cannot leave range progress behind new work.
    fn retry(&mut self) -> bool {
        if self.retries >= MAX_RETRIES {
            return false
        }
        self.retries += 1;
        self.fut =
            self.client.get_account_range_with_priority(self.request.clone(), Priority::High);
        true
    }

    // Verify peer-controlled proofs off the async worker while retaining peer attribution.
    fn start_verification(
        &mut self,
        peer_id: PeerId,
        response: SnapResponse,
    ) -> Result<Option<AccountRangeOutcome>, RequestError> {
        let response = self.account_range_response(response)?;

        if response.accounts.is_empty() && response.proof.is_empty() {
            return if self.request.root_hash == EMPTY_ROOT_HASH {
                Ok(Some(AccountRangeOutcome::Verified(VerifiedAccountRange {
                    accounts: Vec::new(),
                    has_more: false,
                    next: None,
                })))
            } else {
                Ok(Some(AccountRangeOutcome::Unavailable { peer_id }))
            }
        }

        let request = self.request.clone();
        let fut = self.runtime.spawn_blocking(move || verify_account_range(&request, response));
        self.verification = Some(VerificationTask { peer_id, fut });
        Ok(None)
    }

    // Bind replies to the expected request before trusting peer-supplied data.
    fn account_range_response(
        &self,
        response: SnapResponse,
    ) -> Result<AccountRangeMessage, RequestError> {
        let SnapResponse::AccountRange(response) = response else {
            debug!(target: "downloaders::snap", "Expected account range response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request.request_id,
                got = response.request_id,
                "Account range response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        Ok(response)
    }

    // Keep validation and retry accounting together so peers are penalized exactly once.
    fn handle_response(
        &mut self,
        response: PeerRequestResult<SnapResponse>,
    ) -> Result<Option<AccountRangeOutcome>, RequestError> {
        match response {
            Ok(response) => {
                let (peer_id, response) = response.split();
                match self.start_verification(peer_id, response) {
                    Ok(outcome) => Ok(outcome),
                    Err(error) => {
                        debug!(target: "downloaders::snap", ?peer_id, %error, "Invalid account range response");
                        self.client.report_bad_message(peer_id);
                        self.retry().then_some(None).ok_or(error)
                    }
                }
            }
            // A wrong wire response is already penalized by the session.
            Err(error) if error.is_retryable() || error == RequestError::BadResponse => {
                debug!(target: "downloaders::snap", %error, "Account range request failed, retrying");
                self.retry().then_some(None).ok_or(error)
            }
            Err(error) => Err(error),
        }
    }

    // Preserve peer attribution until blocking verification finishes.
    fn poll_verification(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<AccountRangeOutcome>, RequestError>> {
        let verification = self.verification.as_mut().expect("verification task is present");
        let result = ready!(verification.fut.poll_unpin(cx));
        let peer_id = verification.peer_id;
        self.verification = None;

        match result {
            Ok(Ok(range)) => Poll::Ready(Ok(Some(AccountRangeOutcome::Verified(range)))),
            Ok(Err(error)) => {
                debug!(target: "downloaders::snap", ?peer_id, %error, "Invalid account range response");
                self.client.report_bad_message(peer_id);
                Poll::Ready(self.retry().then_some(None).ok_or(error))
            }
            Err(error) => {
                debug!(target: "downloaders::snap", %error, "Account range verification task failed");
                Poll::Ready(Err(RequestError::ChannelClosed))
            }
        }
    }
}

impl<C> Future for AccountRangeDownloader<C>
where
    C: SnapClient + Unpin,
{
    type Output = Result<AccountRangeOutcome, RequestError>;

    // Finish an active verification before accepting another response.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            if this.verification.is_some() {
                match ready!(this.poll_verification(cx)) {
                    Ok(Some(outcome)) => return Poll::Ready(Ok(outcome)),
                    Ok(None) => {}
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }

            let response = ready!(this.fut.poll_unpin(cx));
            match this.handle_response(response) {
                Ok(Some(outcome)) => return Poll::Ready(Ok(outcome)),
                Ok(None) => {}
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

/// The result of an authenticated account-range request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountRangeOutcome {
    /// The peer does not have the requested state root and was not penalized.
    Unavailable {
        /// The peer that answered.
        peer_id: PeerId,
    },
    /// An account range authenticated against the requested state root.
    Verified(VerifiedAccountRange),
}

// Couples blocking proof work with its responder so failures remain attributable.
#[derive(Debug)]
struct VerificationTask {
    // Identifies the responder to penalize if verification rejects the range.
    peer_id: PeerId,
    // Carries the verified result back without blocking the async worker.
    fut: tokio::task::JoinHandle<Result<VerifiedAccountRange, RequestError>>,
}

/// A decoded account range authenticated against a state root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAccountRange {
    /// Accounts in strictly increasing hashed-key order.
    pub accounts: Vec<(B256, TrieAccount)>,
    /// Whether another request may be needed to complete the requested interval.
    ///
    /// Conservative: [`Self::next`] is only a lower bound when the trie continues inside a
    /// subtree the proof left unexpanded, so this can be `true` for an interval that is already
    /// complete.
    pub has_more: bool,
    /// Authenticated lower bound for the first key after the response, or `None` when the range
    /// exhausted the trie.
    pub next: Option<B256>,
}

// Authenticate the full response before trimming its optional boundary account.
fn verify_account_range(
    request: &GetAccountRangeMessage,
    response: AccountRangeMessage,
) -> Result<VerifiedAccountRange, RequestError> {
    // Allow only the single out-of-range account needed as a boundary witness.
    if response.accounts.iter().filter(|data| data.hash > request.limit_hash).nth(1).is_some() {
        debug!(target: "downloaders::snap", "Account range runs past the requested limit");
        return Err(RequestError::BadResponse)
    }

    // Decode first so malformed account values are attributed to the responder.
    let mut accounts = response
        .accounts
        .into_iter()
        .map(|data| {
            data.into_trie_entry().map_err(|error| {
                debug!(target: "downloaders::snap", %error, "Invalid account data");
                RequestError::BadResponse
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let next = verify_proof(request, &accounts, &response.proof)?;

    // Authenticate the boundary account before removing it from the requested range.
    accounts.truncate(accounts.partition_point(|(hash, _)| *hash <= request.limit_hash));
    let has_more = next.is_some_and(|next| next <= request.limit_hash);

    Ok(VerifiedAccountRange { accounts, has_more, next })
}

// Re-encode decoded accounts so the proof authenticates their canonical trie values.
fn verify_proof(
    request: &GetAccountRangeMessage,
    accounts: &[(B256, TrieAccount)],
    proof: &[alloy_primitives::Bytes],
) -> Result<Option<B256>, RequestError> {
    let leaves = accounts.iter().map(|(hash, account)| (*hash, alloy_rlp::encode(account)));
    verify_range_proof(request.root_hash, request.starting_hash, request.limit_hash, leaves, proof)
        .map_err(|error| {
            debug!(target: "downloaders::snap", %error, "Invalid account range proof");
            RequestError::BadResponse
        })
}

/// An account-range request whose origin exceeds its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("account range origin {origin} exceeds limit {limit}")]
pub struct InvalidAccountRange {
    /// Inclusive origin the range was requested from.
    pub origin: B256,
    /// Inclusive limit the range was requested to.
    pub limit: B256,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Bytes, KECCAK256_EMPTY, U256};
    use futures::future::{ready, Ready};
    use reth_eth_wire_types::snap::{
        AccountData, ByteCodesMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
        GetStorageRangesMessage,
    };
    use reth_network_p2p::download::DownloadClient;
    use reth_network_peers::WithPeerId;
    use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

    #[derive(Debug)]
    struct TestSnapClient {
        responses: Mutex<VecDeque<PeerRequestResult<SnapResponse>>>,
        reported: Mutex<Vec<PeerId>>,
        priorities: Mutex<Vec<Priority>>,
    }

    impl TestSnapClient {
        fn new(responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                reported: Mutex::new(Vec::new()),
                priorities: Mutex::new(Vec::new()),
            }
        }

        fn next(&self, priority: Priority) -> Ready<PeerRequestResult<SnapResponse>> {
            self.priorities.lock().unwrap().push(priority);
            ready(self.responses.lock().unwrap().pop_front().expect("test response available"))
        }
    }

    impl DownloadClient for TestSnapClient {
        fn report_bad_message(&self, peer_id: PeerId) {
            self.reported.lock().unwrap().push(peer_id);
        }

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl SnapClient for TestSnapClient {
        type Output = Ready<PeerRequestResult<SnapResponse>>;

        fn get_account_range_with_priority(
            &self,
            _request: GetAccountRangeMessage,
            priority: Priority,
        ) -> Self::Output {
            self.next(priority)
        }

        fn get_storage_ranges(&self, _request: GetStorageRangesMessage) -> Self::Output {
            ready(Err(RequestError::UnsupportedCapability))
        }

        fn get_storage_ranges_with_priority(
            &self,
            _request: GetStorageRangesMessage,
            _priority: Priority,
        ) -> Self::Output {
            ready(Err(RequestError::UnsupportedCapability))
        }

        fn get_byte_codes(&self, _request: GetByteCodesMessage) -> Self::Output {
            ready(Err(RequestError::UnsupportedCapability))
        }

        fn get_byte_codes_with_priority(
            &self,
            _request: GetByteCodesMessage,
            _priority: Priority,
        ) -> Self::Output {
            ready(Err(RequestError::UnsupportedCapability))
        }

        fn get_block_access_lists_with_priority(
            &self,
            _request: GetBlockAccessListsMessage,
            _priority: Priority,
        ) -> Self::Output {
            ready(Err(RequestError::UnsupportedCapability))
        }
    }

    fn key(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    fn account(nonce: u64) -> TrieAccount {
        TrieAccount {
            nonce,
            balance: U256::from(1),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        }
    }

    fn root(accounts: &[(B256, TrieAccount)]) -> B256 {
        let mut builder = HashBuilder::default();
        for (key, account) in accounts {
            builder.add_leaf(Nibbles::unpack(*key), &alloy_rlp::encode(account));
        }
        builder.root()
    }

    fn root_and_proof(accounts: &[(B256, TrieAccount)], targets: &[B256]) -> (B256, Vec<Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (key, account) in accounts {
            builder.add_leaf(Nibbles::unpack(*key), &alloy_rlp::encode(account));
        }
        let root = builder.root();
        let proof = builder
            .take_proof_nodes()
            .into_nodes_sorted()
            .into_iter()
            .map(|(_, node)| node)
            .collect();
        (root, proof)
    }

    fn request(root_hash: B256) -> GetAccountRangeMessage {
        GetAccountRangeMessage {
            request_id: 1,
            root_hash,
            starting_hash: B256::ZERO,
            limit_hash: MAX_HASH,
            response_bytes: 512 * 1024,
        }
    }

    fn response(peer: PeerId, message: AccountRangeMessage) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(peer, SnapResponse::AccountRange(message)))
    }

    fn downloader(
        client: Arc<TestSnapClient>,
        request: GetAccountRangeMessage,
    ) -> Result<AccountRangeDownloader<Arc<TestSnapClient>>, InvalidAccountRange> {
        AccountRangeDownloader::new(client, request, Runtime::test())
    }

    #[test]
    fn verifies_and_decodes_without_an_ambient_runtime() {
        let accounts = vec![(key(1), account(7)), (key(2), account(8))];
        let root_hash = root(&accounts);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: accounts
                .iter()
                .map(|(key, account)| AccountData::from_trie_account(*key, account))
                .collect(),
            proof: Vec::new(),
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));

        let downloader = downloader(Arc::clone(&client), request(root_hash)).unwrap();
        let outcome = futures::executor::block_on(downloader).unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts,
                has_more: false,
                next: None,
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
        assert_eq!(*client.priorities.lock().unwrap(), [Priority::Normal]);
    }

    #[tokio::test]
    async fn invalid_peer_is_reported_and_request_is_retried_at_high_priority() {
        let accounts = vec![(key(1), account(7))];
        let root_hash = root(&accounts);
        let bad_peer = PeerId::random();
        let good_peer = PeerId::random();
        let bad = Ok(WithPeerId::new(
            bad_peer,
            SnapResponse::ByteCodes(ByteCodesMessage { request_id: 1, codes: Vec::new() }),
        ));
        let good = response(
            good_peer,
            AccountRangeMessage {
                request_id: 1,
                accounts: vec![AccountData::from_trie_account(accounts[0].0, &accounts[0].1)],
                proof: Vec::new(),
            },
        );
        let client = Arc::new(TestSnapClient::new([bad, good]));

        let outcome = downloader(Arc::clone(&client), request(root_hash)).unwrap().await.unwrap();

        assert!(matches!(outcome, AccountRangeOutcome::Verified(_)));
        assert_eq!(*client.reported.lock().unwrap(), [bad_peer]);
        assert_eq!(*client.priorities.lock().unwrap(), [Priority::Normal, Priority::High]);
    }

    #[tokio::test]
    async fn unavailable_state_is_not_a_bad_peer_response() {
        let peer = PeerId::random();
        let message =
            AccountRangeMessage { request_id: 1, accounts: Vec::new(), proof: Vec::new() };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));

        let outcome = downloader(Arc::clone(&client), request(B256::repeat_byte(0x11)))
            .unwrap()
            .await
            .unwrap();

        assert_eq!(outcome, AccountRangeOutcome::Unavailable { peer_id: peer });
        assert!(client.reported.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_request_range_is_rejected_before_submission() {
        let client = Arc::new(TestSnapClient::new(std::iter::empty()));
        let mut request = request(B256::repeat_byte(0x11));
        request.starting_hash = key(2);
        request.limit_hash = key(1);

        assert!(matches!(
            downloader(Arc::clone(&client), request),
            Err(InvalidAccountRange { .. })
        ));
        assert!(client.priorities.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn authenticates_then_trims_an_account_past_the_limit() {
        let accounts = vec![(key(1), account(7)), (key(3), account(8)), (key(4), account(9))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(1), key(3)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: accounts[..2]
                .iter()
                .map(|(key, account)| AccountData::from_trie_account(*key, account))
                .collect(),
            proof,
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.limit_hash = key(2);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: false,
                next: Some(key(4)),
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn range_running_past_the_limit_is_rejected() {
        let accounts = vec![(key(1), account(7)), (key(3), account(8)), (key(4), account(9))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(1), key(4)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: accounts
                .iter()
                .map(|(key, account)| AccountData::from_trie_account(*key, account))
                .collect(),
            proof,
        };
        let attempts = usize::from(MAX_RETRIES) + 1;
        let client = Arc::new(TestSnapClient::new(
            std::iter::repeat_with(|| response(peer, message.clone())).take(attempts),
        ));
        let mut request = request(root_hash);
        request.limit_hash = key(2);

        let error = downloader(Arc::clone(&client), request).unwrap().await.unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(client.reported.lock().unwrap().len(), attempts);
    }

    #[tokio::test]
    async fn account_at_the_limit_completes_the_requested_interval() {
        let accounts = vec![(key(1), account(7)), (key(2), account(8)), (key(3), account(9))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(1), key(2)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: accounts[..2]
                .iter()
                .map(|(key, account)| AccountData::from_trie_account(*key, account))
                .collect(),
            proof,
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.limit_hash = key(2);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: accounts[..2].to_vec(),
                has_more: false,
                next: Some(key(3)),
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    // The first account after the limit proves an empty interval.
    #[tokio::test]
    async fn empty_interval_is_proven_by_the_first_account_after_the_limit() {
        let accounts = vec![(key(1), account(7)), (key(9), account(8))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(3), key(9)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: vec![AccountData::from_trie_account(accounts[1].0, &accounts[1].1)],
            proof,
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.starting_hash = key(3);
        request.limit_hash = key(5);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: Vec::new(),
                has_more: false,
                next: None,
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_interval_is_proven_without_a_boundary_account() {
        let accounts = vec![(key(1), account(7)), (key(9), account(8))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(3), key(5)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage { request_id: 1, accounts: Vec::new(), proof };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.starting_hash = key(3);
        request.limit_hash = key(5);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: Vec::new(),
                has_more: false,
                next: Some(key(9)),
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    // A proof that continues past the limit completes the requested interval.
    #[tokio::test]
    async fn range_ending_before_the_limit_needs_no_further_request() {
        let accounts = vec![(key(1), account(7)), (key(9), account(8))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(1)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: vec![AccountData::from_trie_account(accounts[0].0, &accounts[0].1)],
            proof,
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.limit_hash = key(5);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: false,
                next: Some(key(9)),
            })
        );
    }

    #[tokio::test]
    async fn range_ending_before_a_covered_key_reports_more() {
        let accounts = vec![(key(1), account(7)), (key(3), account(8))];
        let (root_hash, proof) = root_and_proof(&accounts, &[key(1)]);
        let peer = PeerId::random();
        let message = AccountRangeMessage {
            request_id: 1,
            accounts: vec![AccountData::from_trie_account(accounts[0].0, &accounts[0].1)],
            proof,
        };
        let client = Arc::new(TestSnapClient::new([response(peer, message)]));
        let mut request = request(root_hash);
        request.limit_hash = key(5);

        let outcome = downloader(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: true,
                next: Some(key(3)),
            })
        );
    }

    #[tokio::test]
    async fn request_errors_retry_without_duplicate_peer_penalties() {
        let accounts = vec![(key(1), account(7))];
        let root_hash = root(&accounts);
        let peer = PeerId::random();
        let good = response(
            peer,
            AccountRangeMessage {
                request_id: 1,
                accounts: vec![AccountData::from_trie_account(accounts[0].0, &accounts[0].1)],
                proof: Vec::new(),
            },
        );
        let client = Arc::new(TestSnapClient::new([
            Err(RequestError::Timeout),
            Err(RequestError::BadResponse),
            good,
        ]));

        downloader(Arc::clone(&client), request(root_hash)).unwrap().await.unwrap();

        assert!(client.reported.lock().unwrap().is_empty());
        assert_eq!(
            *client.priorities.lock().unwrap(),
            [Priority::Normal, Priority::High, Priority::High]
        );
    }

    #[tokio::test]
    async fn stops_after_the_retry_budget_is_exhausted() {
        let peers = [PeerId::random(), PeerId::random(), PeerId::random()];
        let responses = peers.map(|peer| {
            Ok(WithPeerId::new(
                peer,
                SnapResponse::ByteCodes(ByteCodesMessage { request_id: 1, codes: Vec::new() }),
            ))
        });
        let client = Arc::new(TestSnapClient::new(responses));

        let error = downloader(Arc::clone(&client), request(B256::repeat_byte(0x11)))
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(*client.reported.lock().unwrap(), peers);
        assert_eq!(
            *client.priorities.lock().unwrap(),
            [Priority::Normal, Priority::High, Priority::High]
        );
    }
}
