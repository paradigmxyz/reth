//! Downloads block access lists and authenticates them against header commitments.
//!
//! Responses are positional: entry `i` answers the `i`th requested block hash, and is accepted
//! only if it hashes to the commitment carried by that block's header, as defined by
//! [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189).

use super::request::{SnapVerifier, VerifyingRequest};
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::bal::{DecodedBal, RawBal};
use alloy_primitives::{Bytes, Sealable, B256};
use futures::Future;
use reth_eth_wire_types::snap::{BlockAccessListsMessage, GetBlockAccessListsMessage};
use reth_network_p2p::{
    error::RequestError,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_primitives_traits::SealedHeader;
use reth_tasks::Runtime;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tracing::debug;

/// Downloads block access lists and authenticates each against its header commitment.
///
/// Invalid responses penalize their peer and retry. Decoding and hashing run on the blocking
/// pool.
#[derive(Debug)]
pub struct BlockAccessListDownloader<C: SnapClient>(VerifyingRequest<C, BlockAccessListVerifier>);

impl<C: SnapClient> BlockAccessListDownloader<C> {
    /// Creates a downloader that verifies responses against `headers`.
    ///
    /// Headers must match the requested block hashes in order and carry their block-access-list
    /// commitments.
    pub fn new<H: BlockHeader + Sealable>(
        client: C,
        request: GetBlockAccessListsMessage,
        headers: &[SealedHeader<H>],
        runtime: Runtime,
    ) -> Result<Self, InvalidBlockAccessListRequest> {
        if request.block_hashes.is_empty() {
            return Err(InvalidBlockAccessListRequest::NoBlocks)
        }
        if request.block_hashes.len() != headers.len() {
            return Err(InvalidBlockAccessListRequest::HeaderCount {
                requested: request.block_hashes.len(),
                supplied: headers.len(),
            })
        }

        // Only authenticated block identities cross into blocking work, so the verifier stays
        // free of the caller's header type.
        let mut blocks = Vec::with_capacity(headers.len());
        for (index, (requested, header)) in request.block_hashes.iter().zip(headers).enumerate() {
            if *requested != header.hash() {
                return Err(InvalidBlockAccessListRequest::HashMismatch {
                    index,
                    requested: *requested,
                    supplied: header.hash(),
                })
            }
            let Some(commitment) = header.block_access_list_hash() else {
                return Err(InvalidBlockAccessListRequest::MissingCommitment {
                    index,
                    block_hash: *requested,
                })
            };
            blocks.push((*requested, commitment));
        }

        let verifier = BlockAccessListVerifier {
            request_id: request.request_id,
            response_bytes: request.response_bytes,
            blocks,
        };
        Ok(Self(VerifyingRequest::new(client, request, verifier, runtime)))
    }
}

impl<C> Future for BlockAccessListDownloader<C>
where
    C: SnapClient + Unpin,
{
    type Output = Result<BlockAccessListOutcome, RequestError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.poll_verified(cx)
    }
}

/// Result of an authenticated block-access-lists request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockAccessListOutcome {
    /// The peer serves snap but holds none of the requested lists, and was not penalized.
    Unavailable {
        /// The peer that answered.
        peer_id: PeerId,
    },
    /// Lists authenticated against their header commitments.
    Verified(VerifiedBlockAccessLists),
}

/// Positional block access lists authenticated against their header commitments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBlockAccessLists {
    // Needed to avoid retrying unavailable entries against the peer that omitted them.
    peer_id: PeerId,
    // Each list stays bound to the block whose header commitment authenticated it.
    block_access_lists: Vec<(B256, Option<DecodedBal>)>,
    // Requested blocks the response left unanswered, in request order.
    missing: Vec<B256>,
    // Soft byte limit of the answered request, so a follow-up is bounded the same way.
    response_bytes: u64,
}

impl VerifiedBlockAccessLists {
    /// Peer that returned these lists.
    pub const fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Block hashes and lists in requested order, with `None` where the peer had none.
    pub fn block_access_lists(&self) -> &[(B256, Option<DecodedBal>)] {
        &self.block_access_lists
    }

    /// Consumes the result without separating lists from their authenticated block hashes.
    pub fn into_block_access_lists(self) -> Vec<(B256, Option<DecodedBal>)> {
        self.block_access_lists
    }

    /// Request the blocks left unanswered, or `None` once every list is authenticated.
    /// The returned request contains only unanswered hashes, preserving authenticated lists.
    pub fn follow_up(&self, request_id: u64) -> Option<GetBlockAccessListsMessage> {
        (!self.missing.is_empty()).then(|| GetBlockAccessListsMessage {
            request_id,
            block_hashes: self.missing.clone(),
            response_bytes: self.response_bytes,
        })
    }

    /// Requested blocks this response left unanswered, in request order.
    /// This includes in-place omissions and any truncated suffix.
    pub fn missing(&self) -> &[B256] {
        &self.missing
    }
}

/// A block-access-lists request that cannot be authenticated by the headers supplied with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidBlockAccessListRequest {
    /// The request asks for no blocks.
    #[error("block access list request has no block hashes")]
    NoBlocks,
    /// A different number of headers was supplied than blocks requested.
    #[error("requested {requested} block access lists but supplied {supplied} headers")]
    HeaderCount {
        /// Blocks the request asks for.
        requested: usize,
        /// Headers supplied to authenticate them.
        supplied: usize,
    },
    /// A requested hash does not match the header at the same position.
    #[error("requested block {requested} at index {index} but header is for {supplied}")]
    HashMismatch {
        /// Position the mismatch was found at.
        index: usize,
        /// Hash the request asks for.
        requested: B256,
        /// Hash of the header supplied for it.
        supplied: B256,
    },
    /// A supplied header carries no commitment, so its list could never be authenticated.
    #[error("header for block {block_hash} at index {index} has no block access list commitment")]
    MissingCommitment {
        /// Position of the header without a commitment.
        index: usize,
        /// Block the header belongs to.
        block_hash: B256,
    },
}

// Authenticates each returned list against the commitment of the block it answers.
//
// Keeps only response identity and authenticated block pairs, avoiding the caller's generic
// header type on the blocking pool.
#[derive(Clone, Debug)]
struct BlockAccessListVerifier {
    // Matches the response to the request that asked for it.
    request_id: u64,
    // Soft byte limit the request was sent with, carried into any follow-up.
    response_bytes: u64,
    // Requested hashes paired with their header commitments, in wire order.
    blocks: Vec<(B256, B256)>,
}

impl BlockAccessListVerifier {
    // Checks that the response can be paired with this request before its entries are decoded.
    fn validate_response(
        &self,
        response: SnapResponse,
    ) -> Result<BlockAccessListsMessage, RequestError> {
        let SnapResponse::BlockAccessLists(response) = response else {
            debug!(target: "downloaders::snap", "Expected block access lists response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request_id,
                got = response.request_id,
                "Block access lists response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.block_access_lists.0.len() > self.blocks.len() {
            debug!(
                target: "downloaders::snap",
                requested = self.blocks.len(),
                got = response.block_access_lists.0.len(),
                "Block access lists response is longer than the request"
            );
            return Err(RequestError::BadResponse)
        }

        Ok(response)
    }

    // Keeps each decoded entry tied to the commitment at the same request position.
    fn authenticate_entries(
        &self,
        entries: Vec<Option<Bytes>>,
    ) -> Result<Vec<(B256, Option<DecodedBal>)>, RequestError> {
        let mut block_access_lists = Vec::with_capacity(entries.len());
        for (index, entry) in entries.into_iter().enumerate() {
            let (block_hash, commitment) = self.blocks[index];
            // An omitted list stays in place, so every later entry keeps the commitment it
            // answers.
            let Some(raw) = entry else {
                block_access_lists.push((block_hash, None));
                continue
            };
            // Hashing the raw bytes settles authenticity without decoding, so a peer cannot
            // charge us the decode of a list it was never able to serve.
            let raw = RawBal::new(raw);
            if raw.hash() != commitment {
                debug!(
                    target: "downloaders::snap",
                    %block_hash,
                    expected = %commitment,
                    got = %raw.hash(),
                    "Block access list does not match its header commitment"
                );
                return Err(RequestError::BadResponse)
            }
            let decoded = DecodedBal::from_raw_bal(raw).map_err(|error| {
                debug!(target: "downloaders::snap", %block_hash, %error, "Invalid block access list");
                RequestError::BadResponse
            })?;
            block_access_lists.push((block_hash, Some(decoded)));
        }

        Ok(block_access_lists)
    }
}

impl SnapVerifier for BlockAccessListVerifier {
    type Request = GetBlockAccessListsMessage;
    type Output = BlockAccessListOutcome;

    // Validates the response identity and authenticates every supplied block access list.
    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        let entries = self.validate_response(response)?.block_access_lists.0;
        // An empty response is the peer's explicit statement that it has none of these lists.
        if entries.is_empty() {
            return Ok(BlockAccessListOutcome::Unavailable { peer_id })
        }

        let block_access_lists = self.authenticate_entries(entries)?;
        // Omitted entries and a cut at the peer's soft byte limit both leave blocks unanswered,
        // and neither invalidates the entries around them.
        let missing = block_access_lists
            .iter()
            .filter_map(|(block_hash, list)| list.is_none().then_some(*block_hash))
            .chain(
                self.blocks[block_access_lists.len()..].iter().map(|(block_hash, _)| *block_hash),
            )
            .collect::<Vec<_>>();
        // A full response of omitted entries is unavailable, a shorter one can be byte-limited.
        // Preserve the latter's omitted suffix for resumption.
        if block_access_lists.len() == self.blocks.len() && missing.len() == self.blocks.len() {
            return Ok(BlockAccessListOutcome::Unavailable { peer_id })
        }

        Ok(BlockAccessListOutcome::Verified(VerifiedBlockAccessLists {
            peer_id,
            block_access_lists,
            missing,
            response_bytes: self.response_bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{request::MAX_RETRIES, test_utils::TestSnapClient},
        *,
    };
    use alloy_consensus::Header;
    use alloy_eips::eip7928::bal::Bal;
    use alloy_primitives::Bytes;
    use reth_eth_wire_types::{
        snap::{AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage},
        BlockAccessLists,
    };
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use std::sync::Arc;

    fn bal() -> Bytes {
        Bytes::from(alloy_rlp::encode(Bal::default()))
    }

    fn commitment(raw: Bytes) -> B256 {
        DecodedBal::from_rlp_bytes(raw).expect("test bal decodes").hash()
    }

    // Headers committing to `entries`, sealed with distinct block hashes. A `None` entry gets a
    // commitment no list can match, since nothing authenticates against it.
    fn headers(entries: &[Option<Bytes>]) -> Vec<SealedHeader<Header>> {
        entries
            .iter()
            .enumerate()
            .map(|(index, raw)| {
                let header = Header {
                    block_access_list_hash: Some(
                        raw.clone().map_or(B256::repeat_byte(0xee), commitment),
                    ),
                    ..Default::default()
                };
                SealedHeader::new(header, B256::repeat_byte(index as u8 + 1))
            })
            .collect()
    }

    fn request(headers: &[SealedHeader<Header>]) -> GetBlockAccessListsMessage {
        GetBlockAccessListsMessage {
            request_id: 1,
            block_hashes: headers.iter().map(SealedHeader::hash).collect(),
            response_bytes: 512 * 1024,
        }
    }

    fn response(
        peer: PeerId,
        request_id: u64,
        entries: Vec<Option<Bytes>>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer,
            SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                request_id,
                block_access_lists: BlockAccessLists(entries),
            }),
        ))
    }

    // A response that can never authenticate a block access list.
    fn unverifiable(peer: PeerId) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer,
            SnapResponse::ByteCodes(ByteCodesMessage { request_id: 1, codes: Vec::new() }),
        ))
    }

    // Every attempt gets the same answer, so a rejected response exhausts the retry budget.
    fn always(
        peer: PeerId,
        request_id: u64,
        entries: Vec<Option<Bytes>>,
    ) -> impl Iterator<Item = PeerRequestResult<SnapResponse>> {
        std::iter::repeat_with(move || response(peer, request_id, entries.clone()))
            .take(usize::from(MAX_RETRIES) + 1)
    }

    fn downloader(
        client: Arc<TestSnapClient>,
        request: GetBlockAccessListsMessage,
        headers: &[SealedHeader<Header>],
    ) -> Result<BlockAccessListDownloader<Arc<TestSnapClient>>, InvalidBlockAccessListRequest> {
        BlockAccessListDownloader::new(client, request, headers, Runtime::test())
    }

    fn verified(outcome: BlockAccessListOutcome) -> VerifiedBlockAccessLists {
        match outcome {
            BlockAccessListOutcome::Verified(verified) => verified,
            BlockAccessListOutcome::Unavailable { .. } => panic!("expected verified lists"),
        }
    }

    #[tokio::test]
    async fn present_and_omitted_entries_keep_their_positions() {
        let entries = vec![Some(bal()), None, Some(bal())];
        let headers = headers(&entries);
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer, 1, entries)]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert_eq!(verified.peer_id(), peer);
        assert_eq!(
            verified
                .block_access_lists()
                .iter()
                .map(|(block_hash, entry)| (*block_hash, entry.as_ref().map(DecodedBal::hash)))
                .collect::<Vec<_>>(),
            [
                (headers[0].hash(), Some(commitment(bal()))),
                (headers[1].hash(), None),
                (headers[2].hash(), Some(commitment(bal()))),
            ]
        );
        // The list after the omission stays authenticated, so only the gap is asked for again.
        assert_eq!(verified.missing(), [headers[1].hash()]);
        assert_eq!(
            verified.follow_up(2),
            Some(GetBlockAccessListsMessage {
                request_id: 2,
                block_hashes: vec![headers[1].hash()],
                response_bytes: request(&headers).response_bytes,
            })
        );
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn a_complete_response_needs_no_follow_up() {
        let entries = vec![Some(bal()), Some(bal())];
        let headers = headers(&entries);
        let client = Arc::new(TestSnapClient::new([response(PeerId::random(), 1, entries)]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert!(verified.missing().is_empty());
        assert_eq!(verified.follow_up(2), None);
    }

    #[tokio::test]
    async fn a_truncated_response_leaves_the_rest_of_the_request_unanswered() {
        let entries = vec![Some(bal()), Some(bal()), Some(bal())];
        let headers = headers(&entries);
        let client =
            Arc::new(TestSnapClient::new([response(PeerId::random(), 1, entries[..2].to_vec())]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert_eq!(verified.missing(), [headers[2].hash()]);
        assert_eq!(verified.block_access_lists().len(), 2);
        assert_eq!(verified.block_access_lists()[0].0, headers[0].hash());
        assert_eq!(verified.block_access_lists()[1].0, headers[1].hash());
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn an_empty_or_complete_response_holding_no_lists_is_unavailable() {
        let headers = headers(&[Some(bal()), Some(bal())]);
        let peer = PeerId::random();
        for entries in [Vec::new(), vec![None, None]] {
            let client = Arc::new(TestSnapClient::new([response(peer, 1, entries)]));

            let outcome = downloader(Arc::clone(&client), request(&headers), &headers)
                .unwrap()
                .await
                .unwrap();

            assert_eq!(outcome, BlockAccessListOutcome::Unavailable { peer_id: peer });
            assert!(client.reported().is_empty());
        }
    }

    #[tokio::test]
    async fn a_truncated_response_after_an_omission_stays_resumable() {
        let headers = headers(&[Some(bal()), Some(bal())]);
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer, 1, vec![None])]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert_eq!(verified.missing(), [headers[0].hash(), headers[1].hash()]);
        assert_eq!(
            verified.follow_up(2),
            Some(GetBlockAccessListsMessage {
                request_id: 2,
                block_hashes: headers.iter().map(SealedHeader::hash).collect(),
                response_bytes: request(&headers).response_bytes,
            })
        );
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn an_invalid_list_is_reported_and_retried() {
        let entries = vec![Some(bal())];
        let headers = headers(&entries);
        let bad_peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([
            response(bad_peer, 1, vec![Some(Bytes::from_static(&[0xff, 0xff]))]),
            response(PeerId::random(), 1, entries),
        ]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        assert_eq!(verified(outcome).block_access_lists().len(), 1);
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
    }

    #[tokio::test]
    async fn a_list_that_misses_its_header_commitment_is_rejected() {
        // The header commits to something this list cannot hash to.
        let headers = vec![SealedHeader::new(
            Header { block_access_list_hash: Some(B256::repeat_byte(0xab)), ..Default::default() },
            B256::repeat_byte(1),
        )];
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new(always(peer, 1, vec![Some(bal())])));

        let error = downloader(Arc::clone(&client), request(&headers), &headers)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
    }

    #[tokio::test]
    async fn dropping_an_entry_instead_of_omitting_it_in_place_is_rejected() {
        // The peer holds no list for the first block and drops the slot rather than sending
        // `None`, so the second block's list lands on the first block's commitment.
        let headers = headers(&[None, Some(bal())]);
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new(always(peer, 1, vec![Some(bal())])));

        let error = downloader(Arc::clone(&client), request(&headers), &headers)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
    }

    #[tokio::test]
    async fn malformed_wrong_id_and_oversized_responses_are_rejected() {
        let headers = headers(&[Some(bal())]);
        let peer = PeerId::random();
        for (request_id, entries) in [
            // undecodable payload
            (1, vec![Some(Bytes::from_static(&[0xff, 0xff]))]),
            // answers a different request
            (7, vec![Some(bal())]),
            // more entries than blocks requested
            (1, vec![Some(bal()), Some(bal())]),
        ] {
            let client = Arc::new(TestSnapClient::new(always(peer, request_id, entries)));

            let error = downloader(Arc::clone(&client), request(&headers), &headers)
                .unwrap()
                .await
                .unwrap_err();

            assert_eq!(error, RequestError::BadResponse);
            assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
        }
    }

    #[tokio::test]
    async fn a_wrong_response_type_exhausts_the_retry_budget() {
        let headers = headers(&[Some(bal())]);
        let peers = [PeerId::random(), PeerId::random(), PeerId::random()];
        let client = Arc::new(TestSnapClient::new(peers.map(unverifiable)));

        let error = downloader(Arc::clone(&client), request(&headers), &headers)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(*client.reported(), peers);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High, Priority::High]);
    }

    #[test]
    fn requests_that_cannot_be_authenticated_are_rejected_before_submission() {
        let headers = headers(&[Some(bal())]);
        let client = Arc::new(TestSnapClient::new(std::iter::empty()));

        let mut empty = request(&headers);
        empty.block_hashes.clear();
        assert_eq!(
            downloader(Arc::clone(&client), empty, &[]).unwrap_err(),
            InvalidBlockAccessListRequest::NoBlocks
        );

        assert!(matches!(
            downloader(Arc::clone(&client), request(&headers), &[]).unwrap_err(),
            InvalidBlockAccessListRequest::HeaderCount { requested: 1, supplied: 0 }
        ));

        let mut wrong_block = request(&headers);
        wrong_block.block_hashes[0] = B256::repeat_byte(0x99);
        assert!(matches!(
            downloader(Arc::clone(&client), wrong_block, &headers).unwrap_err(),
            InvalidBlockAccessListRequest::HashMismatch { index: 0, .. }
        ));

        let uncommitted = vec![SealedHeader::new(Header::default(), headers[0].hash())];
        assert!(matches!(
            downloader(Arc::clone(&client), request(&headers), &uncommitted).unwrap_err(),
            InvalidBlockAccessListRequest::MissingCommitment { index: 0, .. }
        ));

        // Nothing reached the network.
        assert!(client.priorities().is_empty());
    }

    #[test]
    fn a_response_of_another_kind_is_rejected() {
        let verifier = BlockAccessListVerifier {
            request_id: 1,
            response_bytes: 512 * 1024,
            blocks: vec![(B256::repeat_byte(1), commitment(bal()))],
        };
        let wrong = SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        });

        assert_eq!(verifier.verify(PeerId::random(), wrong), Err(RequestError::BadResponse));
    }
}
