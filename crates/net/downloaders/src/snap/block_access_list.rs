//! Downloads block access lists for Snap/2 state catch-up.
//!
//! Each available response entry is decoded with Alloy and authenticated against its sealed
//! header commitment before it can reach the sync orchestrator.

use super::request::{SnapVerifier, VerifyingRequest};
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::bal::DecodedBal;
use alloy_primitives::{Sealable, B256};
use futures::Future;
use reth_eth_wire_types::snap::GetBlockAccessListsMessage;
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

/// Downloads BALs authenticated by their corresponding sealed headers.
#[derive(Debug)]
pub struct BlockAccessListDownloader<C: SnapClient>(VerifyingRequest<C, BlockAccessListVerifier>);

impl<C: SnapClient> BlockAccessListDownloader<C> {
    /// Validates positional headers before submitting a non-empty request.
    pub fn new<H: BlockHeader + Sealable>(
        client: C,
        request: GetBlockAccessListsMessage,
        headers: &[SealedHeader<H>],
        runtime: Runtime,
    ) -> Result<Self, InvalidBlockAccessListRequest> {
        if request.block_hashes.is_empty() {
            return Err(InvalidBlockAccessListRequest::NoBlockHashes)
        }
        if request.block_hashes.len() != headers.len() {
            return Err(InvalidBlockAccessListRequest::HeaderCount {
                requested: request.block_hashes.len(),
                supplied: headers.len(),
            })
        }

        let mut commitments = Vec::with_capacity(headers.len());
        for (index, (requested, header)) in request.block_hashes.iter().zip(headers).enumerate() {
            if *requested != header.hash() {
                return Err(InvalidBlockAccessListRequest::HeaderMismatch {
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
            commitments.push(commitment);
        }

        let verifier = BlockAccessListVerifier { request: request.clone(), commitments };
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

/// Result of an authenticated BAL request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockAccessListOutcome {
    /// The responder returned no entries for the requested blocks.
    Unavailable {
        /// Peer that returned the empty response.
        peer_id: PeerId,
    },
    /// BAL entries authenticated against their header commitments.
    Verified(VerifiedBlockAccessLists),
}

/// Positional BAL response after decoding and header authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBlockAccessLists {
    /// Peer whose unavailable entries may be retried elsewhere.
    pub peer_id: PeerId,
    /// Requested block hashes paired with decoded BALs or explicit unavailability.
    pub block_access_lists: Vec<(B256, Option<DecodedBal>)>,
    /// First request position omitted by a truncated response.
    pub next_index: Option<usize>,
}

/// A BAL request that does not match its sealed headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidBlockAccessListRequest {
    /// No block hash was requested.
    #[error("block access list request contains no block hashes")]
    NoBlockHashes,
    /// The request and sealed header batch have different lengths.
    #[error(
        "block access list request has {requested} hashes but {supplied} headers were supplied"
    )]
    HeaderCount {
        /// Number of requested block hashes.
        requested: usize,
        /// Number of supplied sealed headers.
        supplied: usize,
    },
    /// A requested block hash differs from its sealed header position.
    #[error(
        "block access list position {index} requests {requested}, but sealed header is {supplied}"
    )]
    HeaderMismatch {
        /// Position of the mismatch.
        index: usize,
        /// Hash in the wire request.
        requested: B256,
        /// Hash of the supplied sealed header.
        supplied: B256,
    },
    /// A header has no BAL commitment and cannot authenticate Snap/2 data.
    #[error("sealed header {block_hash} at position {index} has no block access list commitment")]
    MissingCommitment {
        /// Position of the header.
        index: usize,
        /// Hash of the uncommitted header.
        block_hash: B256,
    },
}

// Owns only hashes needed during blocking decode and authentication.
#[derive(Clone, Debug)]
struct BlockAccessListVerifier {
    // The request supplies positional block hashes and response correlation.
    request: GetBlockAccessListsMessage,
    // Commitments are detached from generic headers before entering the blocking pool.
    commitments: Vec<B256>,
}

impl SnapVerifier for BlockAccessListVerifier {
    type Request = GetBlockAccessListsMessage;
    type Output = BlockAccessListOutcome;

    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        let SnapResponse::BlockAccessLists(response) = response else {
            debug!(target: "downloaders::snap", "Expected block access lists response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request.request_id,
                got = response.request_id,
                "Block access lists response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.block_access_lists.0.is_empty() {
            return Ok(BlockAccessListOutcome::Unavailable { peer_id })
        }
        if response.block_access_lists.0.len() > self.request.block_hashes.len() {
            debug!(target: "downloaders::snap", "Block access lists response exceeds request");
            return Err(RequestError::BadResponse)
        }

        let response_len = response.block_access_lists.0.len();
        let mut block_access_lists = Vec::with_capacity(response_len);
        for (index, raw) in response.block_access_lists.0.into_iter().enumerate() {
            let block_hash = self.request.block_hashes[index];
            let decoded = raw
                .map(|raw| {
                    let decoded = DecodedBal::from_rlp_bytes(raw).map_err(|error| {
                        debug!(target: "downloaders::snap", %block_hash, %error, "Invalid block access list");
                        RequestError::BadResponse
                    })?;
                    decoded.ensure_hash(self.commitments[index]).map_err(|error| {
                        debug!(target: "downloaders::snap", %block_hash, %error, "Block access list hash mismatch");
                        RequestError::BadResponse
                    })?;
                    Ok::<_, RequestError>(decoded)
                })
                .transpose()?;
            block_access_lists.push((block_hash, decoded));
        }

        let next_index = (response_len < self.request.block_hashes.len()).then_some(response_len);
        Ok(BlockAccessListOutcome::Verified(VerifiedBlockAccessLists {
            peer_id,
            block_access_lists,
            next_index,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::{request::MAX_RETRIES, test_utils::TestSnapClient};
    use alloy_consensus::Header;
    use alloy_eips::eip7928::{AccountChanges, BlockAccessList};
    use alloy_primitives::{Address, Bytes};
    use reth_eth_wire_types::{
        snap::{AccountRangeMessage, BlockAccessListsMessage},
        BlockAccessLists,
    };
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use std::sync::Arc;

    fn raw_bal(seed: u8) -> Bytes {
        let accounts: BlockAccessList = vec![AccountChanges {
            address: Address::repeat_byte(seed),
            storage_changes: Vec::new(),
            storage_reads: Vec::new(),
            balance_changes: Vec::new(),
            nonce_changes: Vec::new(),
            code_changes: Vec::new(),
        }];
        alloy_rlp::encode(accounts).into()
    }

    fn header(number: u64, raw_bal: Option<&Bytes>) -> SealedHeader<Header> {
        SealedHeader::seal_slow(Header {
            number,
            block_access_list_hash: raw_bal.map(alloy_primitives::keccak256),
            ..Default::default()
        })
    }

    fn request(headers: &[SealedHeader<Header>]) -> GetBlockAccessListsMessage {
        GetBlockAccessListsMessage {
            request_id: 1,
            block_hashes: headers.iter().map(SealedHeader::hash).collect(),
            response_bytes: 2 * 1024 * 1024,
        }
    }

    fn response(peer_id: PeerId, entries: Vec<Option<Bytes>>) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer_id,
            SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                request_id: 1,
                block_access_lists: BlockAccessLists(entries),
            }),
        ))
    }

    fn downloader(
        client: Arc<TestSnapClient>,
        request: GetBlockAccessListsMessage,
        headers: &[SealedHeader<Header>],
    ) -> Result<BlockAccessListDownloader<Arc<TestSnapClient>>, InvalidBlockAccessListRequest> {
        BlockAccessListDownloader::new(client, request, headers, Runtime::test())
    }

    #[tokio::test]
    async fn verifies_positional_entries_and_truncated_tail() {
        let raw = raw_bal(1);
        let headers = [header(1, Some(&raw)), header(2, Some(&raw)), header(3, Some(&raw))];
        let peer_id = PeerId::random();
        let client =
            Arc::new(TestSnapClient::new([response(peer_id, vec![Some(raw.clone()), None])]));

        let outcome = downloader(client, request(&headers), &headers).unwrap().await.unwrap();

        let BlockAccessListOutcome::Verified(verified) = outcome else {
            panic!("verified response")
        };
        assert_eq!(verified.peer_id, peer_id);
        assert_eq!(verified.block_access_lists[0].0, headers[0].hash());
        assert_eq!(verified.block_access_lists[0].1.as_ref().unwrap().as_raw(), &raw);
        assert_eq!(verified.block_access_lists[1], (headers[1].hash(), None));
        assert_eq!(verified.next_index, Some(2));
    }

    #[tokio::test]
    async fn empty_response_is_unavailable_without_penalty() {
        let raw = raw_bal(1);
        let headers = [header(1, Some(&raw))];
        let peer_id = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer_id, Vec::new())]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        assert_eq!(outcome, BlockAccessListOutcome::Unavailable { peer_id });
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn invalid_bal_is_reported_and_retried() {
        let expected = raw_bal(1);
        let invalid = raw_bal(2);
        let headers = [header(1, Some(&expected))];
        let bad_peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([
            response(bad_peer, vec![Some(invalid)]),
            response(PeerId::random(), vec![Some(expected)]),
        ]));

        let outcome =
            downloader(Arc::clone(&client), request(&headers), &headers).unwrap().await.unwrap();

        assert!(matches!(outcome, BlockAccessListOutcome::Verified(_)));
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
        assert_eq!(*client.exclusions(), [vec![], vec![bad_peer]]);
    }

    #[test]
    fn rejects_request_without_matching_committed_headers() {
        let raw = raw_bal(1);
        let committed = header(1, Some(&raw));
        let uncommitted = header(2, None);
        let client = Arc::new(TestSnapClient::new(std::iter::empty()));

        let empty = GetBlockAccessListsMessage {
            request_id: 1,
            block_hashes: Vec::new(),
            response_bytes: 0,
        };
        assert!(matches!(
            downloader(Arc::clone(&client), empty, &[]),
            Err(InvalidBlockAccessListRequest::NoBlockHashes)
        ));

        let committed_request = request(std::slice::from_ref(&committed));
        assert!(matches!(
            downloader(Arc::clone(&client), committed_request.clone(), &[]),
            Err(InvalidBlockAccessListRequest::HeaderCount { .. })
        ));
        assert!(matches!(
            downloader(Arc::clone(&client), committed_request, std::slice::from_ref(&uncommitted)),
            Err(InvalidBlockAccessListRequest::HeaderMismatch { .. })
        ));

        let request = request(std::slice::from_ref(&uncommitted));
        assert!(matches!(
            downloader(client, request, std::slice::from_ref(&uncommitted)),
            Err(InvalidBlockAccessListRequest::MissingCommitment { .. })
        ));
    }

    #[test]
    fn rejects_wrong_response_shape_and_invalid_rlp() {
        let raw = raw_bal(1);
        let headers = [header(1, Some(&raw))];
        let request = request(&headers);
        let verifier = BlockAccessListVerifier {
            commitments: vec![alloy_primitives::keccak256(&raw)],
            request,
        };
        let peer_id = PeerId::random();
        let wrong_kind = SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        });
        let wrong_id = SnapResponse::BlockAccessLists(BlockAccessListsMessage {
            request_id: 2,
            block_access_lists: BlockAccessLists(vec![Some(raw)]),
        });
        let too_many = SnapResponse::BlockAccessLists(BlockAccessListsMessage {
            request_id: 1,
            block_access_lists: BlockAccessLists(vec![None, None]),
        });
        let invalid_rlp = SnapResponse::BlockAccessLists(BlockAccessListsMessage {
            request_id: 1,
            block_access_lists: BlockAccessLists(vec![Some(Bytes::from_static(&[0xc1, 0xc0]))]),
        });

        assert_eq!(verifier.clone().verify(peer_id, wrong_kind), Err(RequestError::BadResponse));
        assert_eq!(verifier.clone().verify(peer_id, wrong_id), Err(RequestError::BadResponse));
        assert_eq!(verifier.clone().verify(peer_id, too_many), Err(RequestError::BadResponse));
        assert_eq!(verifier.verify(peer_id, invalid_rlp), Err(RequestError::BadResponse));
    }

    #[tokio::test]
    async fn invalid_bal_exhausts_retry_budget() {
        let expected = raw_bal(1);
        let invalid = raw_bal(2);
        let headers = [header(1, Some(&expected))];
        let peer_id = PeerId::random();
        let attempts = usize::from(MAX_RETRIES) + 1;
        let client = Arc::new(TestSnapClient::new(
            std::iter::repeat_with(|| response(peer_id, vec![Some(invalid.clone())]))
                .take(attempts),
        ));

        let error = downloader(Arc::clone(&client), request(&headers), &headers)
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(*client.reported(), vec![peer_id; attempts]);
    }
}
