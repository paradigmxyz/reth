//! Downloads and verifies snap/2 ranges against
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189) pivot state roots.
//!
//! Persistence and range selection are handled by the snap sync orchestrator.

use alloy_primitives::B256;
use futures::Future;
use reth_eth_wire_types::snap::{
    AccountRangeMessage, GetAccountRangeMessage, GetStorageRangesMessage,
};
use reth_network_p2p::{
    error::RequestError,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;
use reth_trie_common::{range_proof::verify_range_proof, TrieAccount, EMPTY_ROOT_HASH};
use std::{
    ops::Range,
    pin::Pin,
    task::{Context, Poll},
};
use tracing::debug;

mod request;
mod storage;
#[cfg(test)]
mod test_utils;

use request::{SnapVerifier, VerifyingRequest};
pub use storage::*;

/// Downloads and verifies one account range against its requested state root.
///
/// Invalid responses penalize their peer and retry at high priority. Proof verification runs on
/// the blocking pool.
#[derive(Debug)]
pub struct AccountRangeDownloader<C: SnapClient>(VerifyingRequest<C, GetAccountRangeMessage>);

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
        let verifier = request.clone();
        Ok(Self(VerifyingRequest::new(client, request, verifier, runtime)))
    }
}

impl<C> Future for AccountRangeDownloader<C>
where
    C: SnapClient + Unpin,
{
    type Output = Result<AccountRangeOutcome, RequestError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.poll_verified(cx)
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

/// A decoded account range authenticated against a state root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAccountRange {
    // Root the accounts were proven against. Private so a range cannot be relabelled with a root
    // that did not authenticate it.
    state_root: B256,
    // Accounts as the response returned them, in the order every positional check assumes.
    accounts: Vec<(B256, TrieAccount)>,
    // Whether the requested interval may continue past this response.
    has_more: bool,
    // First key after the response, or none when the range ran out of trie.
    next: Option<B256>,
}

impl VerifiedAccountRange {
    /// State root the accounts were authenticated against.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Accounts in strictly increasing hashed-key order.
    pub fn accounts(&self) -> &[(B256, TrieAccount)] {
        &self.accounts
    }

    /// Whether another request may be needed to complete the interval.
    ///
    /// Conservative: can be `true` for an interval that is already complete.
    pub const fn has_more(&self) -> bool {
        self.has_more
    }

    /// Authenticated lower bound for the first key after the response, or `None` when the range
    /// exhausted the trie.
    pub const fn next(&self) -> Option<B256> {
        self.next
    }

    /// Borrows the accounts together with the root that authenticated them.
    pub fn batch(&self) -> VerifiedAccountBatch<'_> {
        VerifiedAccountBatch {
            state_root: self.state_root,
            accounts: self.accounts.iter().map(|(hash, account)| (*hash, account)).collect(),
        }
    }

    /// Borrows only the accounts that have storage, together with the root that authenticated
    /// them.
    ///
    /// Accounts without storage are omitted because snap storage responses do not preserve an
    /// outer-list position for them. Empty when no account in the range has storage.
    pub fn storage_batch(&self) -> VerifiedAccountBatch<'_> {
        VerifiedAccountBatch {
            state_root: self.state_root,
            accounts: self
                .accounts
                .iter()
                .filter(|(_, account)| account.storage_root != EMPTY_ROOT_HASH)
                .map(|(hash, account)| (*hash, account))
                .collect(),
        }
    }
}

/// Accounts and the state root they were authenticated against.
///
/// Only obtainable from [`VerifiedAccountRange`], so requests built from it can always be checked
/// against the root the accounts came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAccountBatch<'a> {
    // Root and accounts travel together, so neither can be swapped for another generation's.
    state_root: B256,
    // Accounts the root authenticated, in response order.
    accounts: Vec<(B256, &'a TrieAccount)>,
}

impl<'a> VerifiedAccountBatch<'a> {
    /// State root the accounts were authenticated against.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Accounts in the order the range returned them.
    pub fn accounts(&self) -> &[(B256, &'a TrieAccount)] {
        &self.accounts
    }

    /// Borrows a positional subrange of the batch, so storage can be requested in bounded chunks
    /// without losing the root that authenticated the accounts.
    ///
    /// `None` when the range falls outside the batch.
    pub fn range(&self, range: Range<usize>) -> Option<Self> {
        self.accounts
            .get(range)
            .map(|accounts| Self { state_root: self.state_root, accounts: accounts.to_vec() })
    }

    // Confirms the batch is the one `request` was built from, so every returned range is checked
    // against the root that authenticated its account.
    pub(super) fn verify_batch(
        &self,
        request: &GetStorageRangesMessage,
    ) -> Result<(), InvalidStorageRangeRequest> {
        if request.root_hash != self.state_root {
            return Err(InvalidStorageRangeRequest::StateRootMismatch {
                requested: request.root_hash,
                authenticated: self.state_root,
            })
        }
        if request.account_hashes.len() != self.accounts.len() {
            return Err(InvalidStorageRangeRequest::AccountCount {
                requested: request.account_hashes.len(),
                supplied: self.accounts.len(),
            })
        }
        for (index, (requested, (supplied, _))) in
            request.account_hashes.iter().zip(&self.accounts).enumerate()
        {
            if requested != supplied {
                return Err(InvalidStorageRangeRequest::AccountMismatch {
                    index,
                    requested: *requested,
                    supplied: *supplied,
                })
            }
        }
        Ok(())
    }

    // The accounts from `from` onwards under the same state root, or none if the batch is shorter.
    pub(super) fn slice(mut self, from: usize) -> Option<Self> {
        (from <= self.accounts.len()).then(|| {
            self.accounts.drain(..from);
            self
        })
    }
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

// The request itself carries everything needed to authenticate its response.
impl SnapVerifier for GetAccountRangeMessage {
    type Request = Self;
    type Output = AccountRangeOutcome;

    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        let SnapResponse::AccountRange(response) = response else {
            debug!(target: "downloaders::snap", "Expected account range response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request_id,
                got = response.request_id,
                "Account range response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.accounts.is_empty() && response.proof.is_empty() {
            return if self.root_hash == EMPTY_ROOT_HASH {
                Ok(AccountRangeOutcome::Verified(VerifiedAccountRange {
                    state_root: self.root_hash,
                    accounts: Vec::new(),
                    has_more: false,
                    next: None,
                }))
            } else {
                Ok(AccountRangeOutcome::Unavailable { peer_id })
            }
        }

        verify_account_range(&self, response).map(AccountRangeOutcome::Verified)
    }
}

// Authenticates the full response before trimming its optional boundary account.
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

    Ok(VerifiedAccountRange { state_root: request.root_hash, accounts, has_more, next })
}

// Re-encodes decoded accounts so the proof authenticates their canonical trie values.
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

#[cfg(test)]
mod tests {
    use super::{request::MAX_RETRIES, test_utils::TestSnapClient, *};
    use alloy_primitives::{Bytes, KECCAK256_EMPTY, U256};
    use reth_eth_wire_types::snap::{AccountData, ByteCodesMessage};
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles};
    use std::sync::Arc;

    const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

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
                state_root: root_hash,
                accounts,
                has_more: false,
                next: None,
            })
        );
        assert!(client.reported().is_empty());
        assert_eq!(*client.priorities(), [Priority::Normal]);
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
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
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
        assert!(client.reported().is_empty());
    }

    #[test]
    fn a_subrange_narrows_the_accounts_and_keeps_their_root() {
        let accounts = vec![(key(1), account(7)), (key(2), account(8)), (key(3), account(9))];
        let root_hash = root(&accounts);
        let range = VerifiedAccountRange {
            state_root: root_hash,
            accounts: accounts.clone(),
            has_more: false,
            next: None,
        };

        let batch = range.batch();
        let chunk = batch.range(1..3).expect("chunk is inside the batch");
        let expected =
            accounts[1..3].iter().map(|(hash, account)| (*hash, account)).collect::<Vec<_>>();
        assert_eq!(chunk.accounts(), expected);
        assert_eq!(chunk.state_root(), root_hash);

        assert_eq!(batch.range(2..4), None);
    }

    #[test]
    fn storage_batch_omits_interleaved_accounts_without_storage() {
        let mut first = account(1);
        first.storage_root = B256::repeat_byte(0x11);
        let empty = account(2);
        let mut third = account(3);
        third.storage_root = B256::repeat_byte(0x33);
        let accounts = vec![(key(1), first), (key(2), empty), (key(3), third)];
        let root_hash = root(&accounts);
        let range =
            VerifiedAccountRange { state_root: root_hash, accounts, has_more: false, next: None };

        let batch = range.storage_batch();

        assert_eq!(batch.state_root(), root_hash);
        assert_eq!(
            batch.accounts().iter().map(|(hash, _)| *hash).collect::<Vec<_>>(),
            vec![key(1), key(3)]
        );

        let chunk = batch.range(1..2).expect("chunk is inside the batch");
        assert_eq!(
            chunk.accounts().iter().map(|(hash, _)| *hash).collect::<Vec<_>>(),
            vec![key(3)]
        );
        assert_eq!(chunk.state_root(), root_hash);
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
        assert!(client.priorities().is_empty());
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
                state_root: root_hash,
                accounts: vec![accounts[0]],
                has_more: false,
                next: Some(key(4)),
            })
        );
        assert!(client.reported().is_empty());
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
        assert_eq!(client.reported().len(), attempts);
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
                state_root: root_hash,
                accounts: accounts[..2].to_vec(),
                has_more: false,
                next: Some(key(3)),
            })
        );
        assert!(client.reported().is_empty());
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
                state_root: root_hash,
                accounts: Vec::new(),
                has_more: false,
                next: None,
            })
        );
        assert!(client.reported().is_empty());
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
                state_root: root_hash,
                accounts: Vec::new(),
                has_more: false,
                next: Some(key(9)),
            })
        );
        assert!(client.reported().is_empty());
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
                state_root: root_hash,
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
                state_root: root_hash,
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

        assert!(client.reported().is_empty());
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High, Priority::High]);
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
        assert_eq!(*client.reported(), peers);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High, Priority::High]);
    }
}
