//! Downloads storage ranges and authenticates them against account storage roots.
//!
//! Responses remain positional, and only the final partial range may use the shared boundary
//! proof defined by the Snap protocol.

use super::request::{SnapVerifier, VerifyingRequest};
use alloy_primitives::{B256, U256};
use futures::Future;
use reth_eth_wire_types::snap::{GetStorageRangesMessage, StorageData, StorageRangesMessage};
use reth_network_p2p::{
    error::RequestError,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;
use reth_trie_common::{range_proof::verify_range_proof, TrieAccount};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tracing::debug;

// Keeps storage requests inclusive through the full trie keyspace.
const MAX_HASH: B256 = B256::new([0xff; B256::len_bytes()]);

/// Downloads storage ranges authenticated by a verified account range.
#[derive(Debug)]
pub struct StorageRangeDownloader<C: SnapClient>(VerifyingRequest<C, StorageRangeVerifier>);

impl<C: SnapClient> StorageRangeDownloader<C> {
    /// Validates positional accounts before submitting the request.
    ///
    /// `accounts` are the entries a verified account range authenticated, in the same order as
    /// `request.account_hashes`; their storage roots are what each returned range is checked
    /// against.
    pub fn new(
        client: C,
        request: GetStorageRangesMessage,
        accounts: &[(B256, TrieAccount)],
        runtime: Runtime,
    ) -> Result<Self, InvalidStorageRangeRequest> {
        let origin = request.starting_hash.unwrap_or(B256::ZERO);
        let limit = request.limit_hash.unwrap_or(MAX_HASH);
        if origin > limit {
            return Err(InvalidStorageRangeRequest::ReversedBounds { origin, limit })
        }
        if request.account_hashes.is_empty() {
            return Err(InvalidStorageRangeRequest::NoAccounts)
        }
        if request.account_hashes.len() != accounts.len() {
            return Err(InvalidStorageRangeRequest::AccountCount {
                requested: request.account_hashes.len(),
                supplied: accounts.len(),
            })
        }

        let mut storage_roots = Vec::with_capacity(accounts.len());
        for (index, (requested, (supplied, account))) in
            request.account_hashes.iter().zip(accounts).enumerate()
        {
            if requested != supplied {
                return Err(InvalidStorageRangeRequest::AccountMismatch {
                    index,
                    requested: *requested,
                    supplied: *supplied,
                })
            }
            storage_roots.push(account.storage_root);
        }

        let verifier = StorageRangeVerifier { request: request.clone(), storage_roots };
        Ok(Self(VerifyingRequest::new(client, request, verifier, runtime)))
    }
}

impl<C> Future for StorageRangeDownloader<C>
where
    C: SnapClient + Unpin,
{
    type Output = Result<StorageRangeOutcome, RequestError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.poll_verified(cx)
    }
}

/// Result of an authenticated storage-ranges request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageRangeOutcome {
    /// The responder lacks the requested state or account.
    Unavailable {
        /// Peer that returned the empty response.
        peer_id: PeerId,
    },
    /// Ranges authenticated against their account storage roots.
    Verified(VerifiedStorageRanges),
}

/// Positional storage ranges authenticated against their accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedStorageRanges {
    /// Ranges in request account order.
    pub ranges: Vec<VerifiedStorageRange>,
    /// Position at which a follow-up request must resume.
    pub continuation: Option<StorageRangeContinuation>,
}

/// Decoded storage slots authenticated against one account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedStorageRange {
    /// Hashed address of the owning account.
    pub account_hash: B256,
    /// Non-zero slots in increasing hashed-key order.
    pub slots: Vec<(B256, U256)>,
}

/// Resume position for an incomplete storage-ranges response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageRangeContinuation {
    /// The final returned account remains incomplete.
    Partial {
        /// Position in the original account list.
        account_index: usize,
        /// Hashed address at `account_index`.
        account_hash: B256,
        /// Inclusive slot origin for the next request.
        starting_hash: B256,
    },
    /// A later requested account was not returned.
    NextAccount {
        /// Position in the original account list.
        account_index: usize,
        /// Hashed address at `account_index`.
        account_hash: B256,
    },
}

/// A storage request that does not match its authenticated accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidStorageRangeRequest {
    /// No account was requested.
    #[error("storage range request contains no accounts")]
    NoAccounts,
    /// The inclusive bounds are reversed.
    #[error("storage range origin {origin} exceeds limit {limit}")]
    ReversedBounds {
        /// Requested inclusive origin.
        origin: B256,
        /// Requested inclusive limit.
        limit: B256,
    },
    /// The request and authenticated batch have different lengths.
    #[error("storage range request has {requested} accounts but {supplied} were supplied")]
    AccountCount {
        /// Number of requested accounts.
        requested: usize,
        /// Number of authenticated accounts.
        supplied: usize,
    },
    /// A requested account differs from its authenticated position.
    #[error(
        "storage range account {index} requests {requested}, but authenticated account is {supplied}"
    )]
    AccountMismatch {
        /// Position of the mismatch.
        index: usize,
        /// Hash in the wire request.
        requested: B256,
        /// Hash in the authenticated batch.
        supplied: B256,
    },
}

// Owns the authenticated roots needed by the blocking verifier.
#[derive(Clone, Debug)]
struct StorageRangeVerifier {
    request: GetStorageRangesMessage,
    storage_roots: Vec<B256>,
}

impl SnapVerifier for StorageRangeVerifier {
    type Request = GetStorageRangesMessage;
    type Output = StorageRangeOutcome;

    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        self.verify_response(peer_id, response)
    }
}

impl StorageRangeVerifier {
    // Every returned list must match the account at the same request position.
    fn verify_response(
        &self,
        peer_id: PeerId,
        response: SnapResponse,
    ) -> Result<StorageRangeOutcome, RequestError> {
        let Some(response) = self.accepted_response(response)? else {
            return Ok(StorageRangeOutcome::Unavailable { peer_id })
        };

        // Only the last returned range may carry the response's shared boundary proof.
        let proof_index = (!response.proof.is_empty()).then_some(response.slots.len() - 1);
        let mut ranges = Vec::with_capacity(response.slots.len());
        let mut final_next = None;

        for (index, slots) in response.slots.iter().enumerate() {
            let proof = if proof_index == Some(index) { response.proof.as_slice() } else { &[] };
            let (range, next) = self.verify_range(index, slots, proof)?;
            ranges.push(range);
            final_next = next;
        }

        let continuation = self.continuation(&ranges, final_next);
        Ok(StorageRangeOutcome::Verified(VerifiedStorageRanges { ranges, continuation }))
    }

    // Binds the reply to this request before any peer-supplied data is trusted. `None` means the
    // responder simply lacks the state.
    fn accepted_response(
        &self,
        response: SnapResponse,
    ) -> Result<Option<StorageRangesMessage>, RequestError> {
        let SnapResponse::StorageRanges(mut response) = response else {
            debug!(target: "downloaders::snap", "Expected storage ranges response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request.request_id,
                got = response.request_id,
                "Storage ranges response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.slots.len() > self.request.account_hashes.len() {
            debug!(target: "downloaders::snap", "Storage response contains extra ranges");
            return Err(RequestError::BadResponse)
        }
        if response.slots.is_empty() {
            if response.proof.is_empty() {
                return Ok(None)
            }
            // A boundary proof can authenticate an empty suffix without an encoded inner list.
            response.slots.push(Vec::new());
        }
        Ok(Some(response))
    }

    // Authenticates one account's slots against its storage root, returning the next slot the
    // trie continues at within the requested bounds.
    fn verify_range(
        &self,
        index: usize,
        slots: &[StorageData],
        proof: &[alloy_primitives::Bytes],
    ) -> Result<(VerifiedStorageRange, Option<B256>), RequestError> {
        let account_hash = self.request.account_hashes[index];
        // Only the first account inherits the request bounds; the rest are whole tries.
        let (origin, limit) = if index == 0 {
            (
                self.request.starting_hash.unwrap_or(B256::ZERO),
                self.request.limit_hash.unwrap_or(MAX_HASH),
            )
        } else {
            (B256::ZERO, MAX_HASH)
        };

        // One boundary slot may sit past the inclusive limit and is verified before removal.
        if slots.iter().filter(|slot| slot.hash > limit).nth(1).is_some() {
            debug!(target: "downloaders::snap", %account_hash, "Storage range exceeds limit");
            return Err(RequestError::BadResponse)
        }

        let mut decoded = Self::decode_slots(account_hash, origin, slots)?;
        let leaves = decoded.iter().map(|(hash, value)| (*hash, alloy_rlp::encode(value)));
        let next = verify_range_proof(self.storage_roots[index], origin, limit, leaves, proof)
            .map_err(|error| {
                debug!(
                    target: "downloaders::snap",
                    %account_hash,
                    %error,
                    "Invalid storage range proof"
                );
                RequestError::BadResponse
            })?;

        decoded.truncate(decoded.partition_point(|(hash, _)| *hash <= limit));
        Ok((
            VerifiedStorageRange { account_hash, slots: decoded },
            next.filter(|next| *next <= limit),
        ))
    }

    // A response is incomplete either part way through its last account, or because a requested
    // account was omitted entirely.
    fn continuation(
        &self,
        ranges: &[VerifiedStorageRange],
        final_next: Option<B256>,
    ) -> Option<StorageRangeContinuation> {
        if let Some(starting_hash) = final_next {
            return Some(StorageRangeContinuation::Partial {
                account_index: ranges.len() - 1,
                account_hash: ranges.last().expect("a response range exists").account_hash,
                starting_hash,
            })
        }
        let account_index = ranges.len();
        self.request.account_hashes.get(account_index).copied().map(|account_hash| {
            StorageRangeContinuation::NextAccount { account_index, account_hash }
        })
    }

    // Storage values are canonical non-zero trie leaves.
    fn decode_slots(
        account_hash: B256,
        origin: B256,
        slots: &[StorageData],
    ) -> Result<Vec<(B256, U256)>, RequestError> {
        let mut decoded = Vec::with_capacity(slots.len());
        let mut previous = None;

        for slot in slots {
            if slot.hash < origin || previous.is_some_and(|previous| slot.hash <= previous) {
                debug!(
                    target: "downloaders::snap",
                    %account_hash,
                    "Storage slots precede origin or are not strictly ordered"
                );
                return Err(RequestError::BadResponse)
            }
            let value = slot.value().map_err(|error| {
                debug!(target: "downloaders::snap", %account_hash, %error, "Invalid storage value");
                RequestError::BadResponse
            })?;
            if value.is_zero() {
                debug!(target: "downloaders::snap", %account_hash, "Storage trie contains zero leaf");
                return Err(RequestError::BadResponse)
            }
            previous = Some(slot.hash);
            decoded.push((slot.hash, value));
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{request::MAX_RETRIES, test_utils::TestSnapClient},
        *,
    };
    use alloy_primitives::{Bytes, KECCAK256_EMPTY};
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles, EMPTY_ROOT_HASH};
    use std::sync::Arc;

    fn key(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    // Storage leaves are RLP-encoded non-zero integers, so the trie value is the encoded slot.
    fn slots(values: &[(B256, u64)]) -> Vec<(B256, U256)> {
        values.iter().map(|(hash, value)| (*hash, U256::from(*value))).collect()
    }

    fn storage_root(slots: &[(B256, U256)], targets: &[B256]) -> (B256, Vec<Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (hash, value) in slots {
            builder.add_leaf(Nibbles::unpack(*hash), &alloy_rlp::encode(value));
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

    fn account(storage_root: B256) -> TrieAccount {
        TrieAccount { nonce: 1, balance: U256::from(2), storage_root, code_hash: KECCAK256_EMPTY }
    }

    fn wire_slots(slots: &[(B256, U256)]) -> Vec<StorageData> {
        slots.iter().map(|(hash, value)| StorageData::from_value(*hash, *value)).collect()
    }

    fn request(accounts: &[(B256, TrieAccount)]) -> GetStorageRangesMessage {
        GetStorageRangesMessage {
            request_id: 1,
            root_hash: B256::repeat_byte(0xaa),
            account_hashes: accounts.iter().map(|(hash, _)| *hash).collect(),
            starting_hash: B256::ZERO.into(),
            limit_hash: MAX_HASH.into(),
            response_bytes: 512 * 1024,
        }
    }

    fn response(
        peer: PeerId,
        request_id: u64,
        slots: Vec<Vec<StorageData>>,
        proof: Vec<Bytes>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer,
            SnapResponse::StorageRanges(StorageRangesMessage { request_id, slots, proof }),
        ))
    }

    fn rejecting_client(
        response: impl Fn(PeerId) -> PeerRequestResult<SnapResponse>,
    ) -> Arc<TestSnapClient> {
        let peer = PeerId::random();
        Arc::new(TestSnapClient::new((0..=MAX_RETRIES).map(|_| response(peer))))
    }

    fn downloader<C: SnapClient>(
        client: C,
        request: GetStorageRangesMessage,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<StorageRangeDownloader<C>, InvalidStorageRangeRequest> {
        StorageRangeDownloader::new(client, request, accounts, Runtime::test())
    }

    #[tokio::test]
    async fn complete_ranges_for_multiple_accounts_are_verified() {
        let first = slots(&[(key(1), 11), (key(2), 12)]);
        let second = slots(&[(key(3), 13)]);
        let (first_root, _) = storage_root(&first, &[]);
        let (second_root, proof) = storage_root(&second, &[key(3)]);
        let accounts = vec![(key(100), account(first_root)), (key(200), account(second_root))];
        let client = Arc::new(TestSnapClient::new([response(
            PeerId::random(),
            1,
            vec![wire_slots(&first), wire_slots(&second)],
            proof,
        )]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        let StorageRangeOutcome::Verified(verified) = outcome else { panic!("verified ranges") };
        assert_eq!(verified.ranges.len(), 2);
        assert_eq!(verified.ranges[0].slots, first);
        assert_eq!(verified.ranges[1].slots, second);
        assert_eq!(verified.continuation, None);
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn partial_final_range_reports_its_slot_continuation() {
        let all = slots(&[(key(1), 11), (key(2), 12), (key(3), 13)]);
        let (root, proof) = storage_root(&all, &[B256::ZERO, key(1)]);
        let accounts = vec![(key(100), account(root))];
        let client = Arc::new(TestSnapClient::new([response(
            PeerId::random(),
            1,
            vec![wire_slots(&all[..1])],
            proof,
        )]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        let StorageRangeOutcome::Verified(verified) = outcome else { panic!("verified ranges") };
        assert_eq!(verified.ranges[0].slots, all[..1]);
        assert_eq!(
            verified.continuation,
            Some(StorageRangeContinuation::Partial {
                account_index: 0,
                account_hash: key(100),
                starting_hash: key(2),
            })
        );
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn an_omitted_account_resumes_at_the_next_position() {
        let first = slots(&[(key(1), 11)]);
        let (first_root, _) = storage_root(&first, &[]);
        let accounts = vec![(key(100), account(first_root)), (key(200), account(EMPTY_ROOT_HASH))];
        let client = Arc::new(TestSnapClient::new([response(
            PeerId::random(),
            1,
            vec![wire_slots(&first)],
            Vec::new(),
        )]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        let StorageRangeOutcome::Verified(verified) = outcome else { panic!("verified ranges") };
        assert_eq!(verified.ranges.len(), 1);
        assert_eq!(
            verified.continuation,
            Some(StorageRangeContinuation::NextAccount {
                account_index: 1,
                account_hash: key(200),
            })
        );
    }

    #[tokio::test]
    async fn an_empty_storage_trie_verifies_without_slots() {
        let accounts = vec![(key(100), account(EMPTY_ROOT_HASH))];
        let client = Arc::new(TestSnapClient::new([response(
            PeerId::random(),
            1,
            vec![Vec::new()],
            Vec::new(),
        )]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        let StorageRangeOutcome::Verified(verified) = outcome else { panic!("verified ranges") };
        assert!(verified.ranges[0].slots.is_empty());
        assert_eq!(verified.continuation, None);
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn a_response_without_slots_or_proof_reports_the_state_unavailable() {
        let accounts = vec![(key(100), account(B256::repeat_byte(0xbb)))];
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer, 1, Vec::new(), Vec::new())]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        assert_eq!(outcome, StorageRangeOutcome::Unavailable { peer_id: peer });
        // Lacking the state is not misbehaviour.
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn a_mismatched_request_id_exhausts_the_retry_budget() {
        let all = slots(&[(key(1), 11)]);
        let (root, _) = storage_root(&all, &[]);
        let accounts = vec![(key(100), account(root))];

        let client = rejecting_client(|peer| response(peer, 9, vec![wire_slots(&all)], Vec::new()));
        let error = downloader(Arc::clone(&client), request(&accounts), &accounts)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        // One penalty per attempt, then the budget is spent.
        assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
    }

    #[tokio::test]
    async fn a_range_proved_against_another_root_is_rejected() {
        let all = slots(&[(key(1), 11)]);
        let accounts = vec![(key(100), account(B256::repeat_byte(0xcc)))];

        let client = rejecting_client(|peer| response(peer, 1, vec![wire_slots(&all)], Vec::new()));
        let error = downloader(Arc::clone(&client), request(&accounts), &accounts)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
    }

    #[tokio::test]
    async fn slots_that_are_not_strictly_ordered_penalize_the_peer_before_a_retry_succeeds() {
        let all = slots(&[(key(1), 11), (key(2), 12)]);
        let (root, _) = storage_root(&all, &[]);
        let accounts = vec![(key(100), account(root))];
        let bad_peer = PeerId::random();
        let good_peer = PeerId::random();
        let mut reversed = wire_slots(&all);
        reversed.reverse();
        let client = Arc::new(TestSnapClient::new([
            response(bad_peer, 1, vec![reversed], Vec::new()),
            response(good_peer, 1, vec![wire_slots(&all)], Vec::new()),
        ]));

        let outcome =
            downloader(Arc::clone(&client), request(&accounts), &accounts).unwrap().await.unwrap();

        let StorageRangeOutcome::Verified(verified) = outcome else { panic!("verified ranges") };
        assert_eq!(verified.ranges[0].slots, all);
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
    }

    #[test]
    fn a_request_that_does_not_match_its_accounts_is_refused() {
        let accounts = vec![(key(100), account(EMPTY_ROOT_HASH))];
        let client = TestSnapClient::new([]);

        let mut mismatched = request(&accounts);
        mismatched.account_hashes = vec![key(999)];
        let error = downloader(&client, mismatched, &accounts).unwrap_err();
        assert_eq!(
            error,
            InvalidStorageRangeRequest::AccountMismatch {
                index: 0,
                requested: key(999),
                supplied: key(100),
            }
        );

        let mut empty = request(&accounts);
        empty.account_hashes.clear();
        assert_eq!(
            downloader(&client, empty, &accounts).unwrap_err(),
            InvalidStorageRangeRequest::NoAccounts
        );

        let mut reversed = request(&accounts);
        reversed.starting_hash = key(9).into();
        reversed.limit_hash = key(1).into();
        assert_eq!(
            downloader(&client, reversed, &accounts).unwrap_err(),
            InvalidStorageRangeRequest::ReversedBounds { origin: key(9), limit: key(1) }
        );
    }
}
