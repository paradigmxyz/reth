//! Downloads and authenticates snap/2 account ranges against [EIP-8189] pivot state roots.
//! Verified ranges report whether another request is needed for the requested interval;
//! persistence and sync orchestration are handled by callers.
//!
//! [EIP-8189]: https://eips.ethereum.org/EIPS/eip-8189

use alloy_primitives::B256;
use futures::{Future, FutureExt};
use reth_eth_wire_types::snap::GetAccountRangeMessage;
use reth_network_p2p::{
    error::RequestError,
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_trie_common::{range_proof::verify_range_proof, TrieAccount, EMPTY_ROOT_HASH};
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// Number of retry attempts after the initial account-range request fails.
const MAX_RETRIES: u8 = 2;

/// Downloads and verifies one account range against its requested state root.
///
/// Invalid peer responses are reported and the same request is retried with high priority. The
/// future is storage agnostic: persisting a verified range and choosing the next range are left to
/// the snap sync orchestrator.
#[derive(Debug)]
pub struct AccountRangeDownloader<C: SnapClient> {
    client: C,
    request: GetAccountRangeMessage,
    fut: C::Output,
    retries: u8,
}

impl<C: SnapClient> AccountRangeDownloader<C> {
    /// Validates the range, then creates a downloader and submits `request` at normal priority.
    pub fn new(client: C, request: GetAccountRangeMessage) -> Result<Self, InvalidAccountRange> {
        if request.starting_hash > request.limit_hash {
            return Err(InvalidAccountRange {
                origin: request.starting_hash,
                limit: request.limit_hash,
            })
        }
        let fut = client.get_account_range(request.clone());
        Ok(Self { client, request, fut, retries: 0 })
    }

    /// Reissues the request at high priority if its retry budget is not exhausted.
    fn retry(&mut self) -> bool {
        if self.retries >= MAX_RETRIES {
            return false
        }
        self.retries += 1;
        self.fut =
            self.client.get_account_range_with_priority(self.request.clone(), Priority::High);
        true
    }

    /// Decodes and verifies a response from a peer.
    fn verify_response(
        &self,
        peer_id: PeerId,
        response: SnapResponse,
    ) -> Result<AccountRangeOutcome, RequestError> {
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

        if response.accounts.is_empty() && response.proof.is_empty() {
            return if self.request.root_hash == EMPTY_ROOT_HASH {
                Ok(AccountRangeOutcome::Verified(VerifiedAccountRange {
                    accounts: Vec::new(),
                    has_more: false,
                }))
            } else {
                Ok(AccountRangeOutcome::Unavailable { peer_id })
            }
        }

        // A responder appends the first account past the limit to prove the interval is complete.
        // Anything beyond that was not requested, so it is rejected before the range is decoded
        // and hashed rather than trimmed away after the work is done.
        if response
            .accounts
            .iter()
            .filter(|data| data.hash > self.request.limit_hash)
            .nth(1)
            .is_some()
        {
            debug!(target: "downloaders::snap", "Account range runs past the requested limit");
            return Err(RequestError::BadResponse)
        }

        let mut accounts = Vec::with_capacity(response.accounts.len());
        for data in response.accounts {
            let hash = data.hash;
            let account = data.trie_account().map_err(|error| {
                debug!(target: "downloaders::snap", %error, "Invalid account data");
                RequestError::BadResponse
            })?;
            accounts.push((hash, account));
        }

        let leaves = accounts.iter().map(|(hash, account)| (*hash, alloy_rlp::encode(account)));
        let next = verify_range_proof(
            self.request.root_hash,
            self.request.starting_hash,
            leaves,
            &response.proof,
        )
        .map_err(|error| {
            debug!(target: "downloaders::snap", %error, "Invalid account range proof");
            RequestError::BadResponse
        })?;

        // Responders append the boundary account before checking the requested limit. Authenticate
        // an overshooting account as part of the response before removing it.
        accounts.truncate(accounts.partition_point(|(hash, _)| *hash <= self.request.limit_hash));

        // The proof pins where the trie continues, so the interval is complete unless a key the
        // response did not cover can still fall inside it.
        let has_more = next.is_some_and(|next| next <= self.request.limit_hash);

        Ok(AccountRangeOutcome::Verified(VerifiedAccountRange { accounts, has_more }))
    }
}

impl<C> Future for AccountRangeDownloader<C>
where
    C: SnapClient + Unpin + 'static,
{
    type Output = Result<AccountRangeOutcome, RequestError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.fut.poll_unpin(cx)) {
                Ok(response) => {
                    let (peer_id, response) = response.split();
                    match this.verify_response(peer_id, response) {
                        Ok(outcome) => return Poll::Ready(Ok(outcome)),
                        Err(error) => {
                            debug!(target: "downloaders::snap", ?peer_id, %error, "Invalid account range response");
                            this.client.report_bad_message(peer_id);
                            if !this.retry() {
                                return Poll::Ready(Err(error))
                            }
                        }
                    }
                }
                // A wrong wire response is already attributed and penalized by the session. It is
                // still safe to retry the request with another snap peer.
                Err(error) if error.is_retryable() || error == RequestError::BadResponse => {
                    debug!(target: "downloaders::snap", %error, "Account range request failed, retrying");
                    if !this.retry() {
                        return Poll::Ready(Err(error))
                    }
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }
}

/// The result of an authenticated account-range request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountRangeOutcome {
    /// The selected peer does not have the requested state root.
    ///
    /// This is not a protocol violation and does not affect peer reputation. The peer is named so
    /// the orchestrator can retry elsewhere, deprioritize it, or advance its pivot.
    Unavailable {
        /// Peer that answered without the requested state.
        peer_id: PeerId,
    },
    /// An account range authenticated against the requested state root.
    Verified(VerifiedAccountRange),
}

/// A decoded account range authenticated against a state root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAccountRange {
    /// Accounts in strictly increasing hashed-key order.
    pub accounts: Vec<(B256, TrieAccount)>,
    /// Whether another request is needed to complete the requested interval.
    pub has_more: bool,
}

/// Error returned when an account-range request has reversed bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("account range origin {origin} exceeds limit {limit}")]
pub struct InvalidAccountRange {
    origin: B256,
    limit: B256,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Bytes, KECCAK256_EMPTY, U256};
    use futures::future::{ready, Ready};
    use reth_eth_wire_types::snap::{
        AccountData, AccountRangeMessage, ByteCodesMessage, GetBlockAccessListsMessage,
        GetByteCodesMessage, GetStorageRangesMessage,
    };
    use reth_network_p2p::{download::DownloadClient, error::PeerRequestResult};
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

    #[tokio::test]
    async fn verifies_and_decodes_a_complete_account_range() {
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

        let outcome = AccountRangeDownloader::new(Arc::clone(&client), request(root_hash))
            .unwrap()
            .await
            .unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange { accounts, has_more: false })
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

        let outcome = AccountRangeDownloader::new(Arc::clone(&client), request(root_hash))
            .unwrap()
            .await
            .unwrap();

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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request(B256::repeat_byte(0x11)))
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
            AccountRangeDownloader::new(Arc::clone(&client), request),
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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: false,
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

        let error =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap_err();

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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: accounts[..2].to_vec(),
                has_more: false,
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    /// snap/2 requires a peer with no account inside `[origin, limit]` to return the first account
    /// after `limit`, so the sole returned account authenticates the interval and then trims away.
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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: Vec::new(),
                has_more: false,
            })
        );
        assert!(client.reported.lock().unwrap().is_empty());
    }

    /// A response cut short by the responder's byte budget completes the interval anyway when the
    /// proof shows the trie continues past the limit.
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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: false,
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

        let outcome =
            AccountRangeDownloader::new(Arc::clone(&client), request).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            AccountRangeOutcome::Verified(VerifiedAccountRange {
                accounts: vec![accounts[0]],
                has_more: true,
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

        AccountRangeDownloader::new(Arc::clone(&client), request(root_hash))
            .unwrap()
            .await
            .unwrap();

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

        let error =
            AccountRangeDownloader::new(Arc::clone(&client), request(B256::repeat_byte(0x11)))
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
