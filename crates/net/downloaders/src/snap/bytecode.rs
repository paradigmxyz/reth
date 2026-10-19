//! Downloads contract bytecode and authenticates it by hash.
//!
//! A peer may omit code it does not have, so a response is an ordered subsequence of the
//! requested hashes, as defined by
//! [snap](https://github.com/ethereum/devp2p/blob/master/caps/snap.md#bytecodes-0x05).

use super::request::{SnapVerifier, VerifyingRequest};
use alloy_primitives::{keccak256, Bytes, B256};
use futures::Future;
use reth_eth_wire_types::snap::GetByteCodesMessage;
use reth_network_p2p::{
    error::RequestError,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tracing::debug;

/// Downloads contract bytecode and authenticates each blob against a requested hash.
///
/// Invalid responses penalize their peer and retry. Hashing runs on the blocking pool.
#[derive(Debug)]
pub struct BytecodeDownloader<C: SnapClient>(VerifyingRequest<C, BytecodeVerifier>);

impl<C: SnapClient> BytecodeDownloader<C> {
    /// Submits `request`, rejecting one that asks for no code.
    pub fn new(
        client: C,
        request: GetByteCodesMessage,
        runtime: Runtime,
    ) -> Result<Self, InvalidBytecodeRequest> {
        if request.hashes.is_empty() {
            return Err(InvalidBytecodeRequest::NoHashes)
        }

        let verifier =
            BytecodeVerifier { request_id: request.request_id, hashes: request.hashes.clone() };
        Ok(Self(VerifyingRequest::new(client, request, verifier, runtime)))
    }
}

impl<C> Future for BytecodeDownloader<C>
where
    C: SnapClient + Unpin,
{
    type Output = Result<BytecodeOutcome, RequestError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.poll_verified(cx)
    }
}

/// Result of an authenticated bytecode request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BytecodeOutcome {
    /// The peer holds none of the requested code, and was not penalized.
    Unavailable {
        /// The peer that answered.
        peer_id: PeerId,
    },
    /// Code authenticated against the requested hashes.
    Verified(VerifiedBytecode),
}

/// Contract code authenticated against the hashes that were requested for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBytecode {
    // Identifies the peer that omitted any missing code.
    peer_id: PeerId,
    // Code paired with the requested hash it answers, in requested order. Private so a blob
    // cannot be relabelled with a hash that did not authenticate it.
    codes: Vec<(B256, Bytes)>,
}

impl VerifiedBytecode {
    /// Peer that returned this code.
    pub const fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Code with the hash it authenticated against, in requested order.
    pub fn codes(&self) -> &[(B256, Bytes)] {
        &self.codes
    }

    /// Consumes the result and returns the authenticated code.
    pub fn into_codes(self) -> Vec<(B256, Bytes)> {
        self.codes
    }

    /// Requested hashes the response did not answer, in requested order.
    ///
    /// Omissions are not a fault: a peer serves what it has, and cuts the response at its own
    /// soft byte limit. These hashes can be asked for again.
    pub fn missing<'a>(&'a self, requested: &'a [B256]) -> impl Iterator<Item = B256> + 'a {
        requested.iter().copied().filter(|hash| !self.codes.iter().any(|(got, _)| got == hash))
    }
}

/// A bytecode request that can never be answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidBytecodeRequest {
    /// The request asks for no code hashes.
    #[error("bytecode request has no code hashes")]
    NoHashes,
}

// Authenticates returned code by hashing it against the requested hashes.
//
// Holds only what blocking verification needs, so nothing else crosses onto the blocking pool.
#[derive(Clone, Debug)]
struct BytecodeVerifier {
    // Matches the response to the request that asked for it.
    request_id: u64,
    // Requested hashes, in the order a response must follow.
    hashes: Vec<B256>,
}

impl SnapVerifier for BytecodeVerifier {
    type Request = GetByteCodesMessage;
    type Output = BytecodeOutcome;

    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        let SnapResponse::ByteCodes(response) = response else {
            debug!(target: "downloaders::snap", "Expected byte codes response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request_id,
                got = response.request_id,
                "Byte codes response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.codes.len() > self.hashes.len() {
            debug!(
                target: "downloaders::snap",
                requested = self.hashes.len(),
                got = response.codes.len(),
                "Byte codes response is longer than the request"
            );
            return Err(RequestError::BadResponse)
        }
        // Serving nothing is a valid answer from a peer that has none of this code.
        if response.codes.is_empty() {
            return Ok(BytecodeOutcome::Unavailable { peer_id })
        }

        // The cursor only moves forward, so code that repeats, reorders, or was never requested
        // finds no hash left to match.
        let mut remaining = self.hashes.as_slice();
        let mut codes = Vec::with_capacity(response.codes.len());
        for code in response.codes {
            let hash = keccak256(&code);
            let Some(offset) = remaining.iter().position(|requested| *requested == hash) else {
                debug!(target: "downloaders::snap", %hash, "Unrequested or out-of-order bytecode");
                return Err(RequestError::BadResponse)
            };
            remaining = &remaining[offset + 1..];
            codes.push((hash, code));
        }

        Ok(BytecodeOutcome::Verified(VerifiedBytecode { peer_id, codes }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{request::MAX_RETRIES, test_utils::TestSnapClient},
        *,
    };
    use reth_eth_wire_types::snap::{AccountRangeMessage, ByteCodesMessage};
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use std::sync::Arc;

    fn code(byte: u8) -> Bytes {
        Bytes::from(vec![byte; 4])
    }

    fn request(codes: &[Bytes]) -> GetByteCodesMessage {
        GetByteCodesMessage {
            request_id: 1,
            hashes: codes.iter().map(keccak256).collect(),
            response_bytes: 512 * 1024,
        }
    }

    fn response(
        peer: PeerId,
        request_id: u64,
        codes: Vec<Bytes>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(peer, SnapResponse::ByteCodes(ByteCodesMessage { request_id, codes })))
    }

    // Every attempt gets the same answer, so a rejected response exhausts the retry budget.
    fn always(
        peer: PeerId,
        request_id: u64,
        codes: Vec<Bytes>,
    ) -> impl Iterator<Item = PeerRequestResult<SnapResponse>> {
        std::iter::repeat_with(move || response(peer, request_id, codes.clone()))
            .take(usize::from(MAX_RETRIES) + 1)
    }

    fn downloader(
        client: Arc<TestSnapClient>,
        request: GetByteCodesMessage,
    ) -> Result<BytecodeDownloader<Arc<TestSnapClient>>, InvalidBytecodeRequest> {
        BytecodeDownloader::new(client, request, Runtime::test())
    }

    fn verified(outcome: BytecodeOutcome) -> VerifiedBytecode {
        match outcome {
            BytecodeOutcome::Verified(verified) => verified,
            BytecodeOutcome::Unavailable { .. } => panic!("expected verified code"),
        }
    }

    #[tokio::test]
    async fn every_requested_code_is_authenticated_by_its_hash() {
        let codes = vec![code(1), code(2), code(3)];
        let client = Arc::new(TestSnapClient::new([response(PeerId::random(), 1, codes.clone())]));

        let outcome = downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert_eq!(
            verified.codes(),
            codes.iter().map(|code| (keccak256(code), code.clone())).collect::<Vec<_>>()
        );
        assert!(verified.missing(&request(&codes).hashes).next().is_none());
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn an_ordered_subsequence_reports_the_omitted_hashes() {
        let codes = vec![code(1), code(2), code(3)];
        let request = request(&codes);
        // The peer serves only the first and last of the three.
        let served = vec![codes[0].clone(), codes[2].clone()];
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer, 1, served)]));

        let outcome = downloader(Arc::clone(&client), request.clone()).unwrap().await.unwrap();

        let verified = verified(outcome);
        assert_eq!(verified.peer_id(), peer);
        assert_eq!(
            verified.codes().iter().map(|(hash, _)| *hash).collect::<Vec<_>>(),
            [keccak256(&codes[0]), keccak256(&codes[2])]
        );
        assert_eq!(verified.missing(&request.hashes).collect::<Vec<_>>(), [keccak256(&codes[1])]);
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn an_empty_response_is_unavailable_and_not_a_peer_fault() {
        let codes = vec![code(1)];
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer, 1, Vec::new())]));

        let outcome = downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap();

        assert_eq!(outcome, BytecodeOutcome::Unavailable { peer_id: peer });
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn unrequested_code_is_reported_and_retried_against_another_peer() {
        let codes = vec![code(1)];
        let bad_peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([
            response(bad_peer, 1, vec![code(9)]),
            response(PeerId::random(), 1, codes.clone()),
        ]));

        let outcome = downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap();

        assert_eq!(verified(outcome).codes().len(), 1);
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
    }

    #[tokio::test]
    async fn out_of_order_repeated_and_oversized_responses_are_rejected() {
        let codes = vec![code(1), code(2)];
        let peer = PeerId::random();
        for served in [
            // reversed, so the second hash is already behind the cursor
            vec![codes[1].clone(), codes[0].clone()],
            // the same code twice, which only one hash was requested for
            vec![codes[0].clone(), codes[0].clone()],
            // more blobs than hashes requested
            vec![codes[0].clone(), codes[1].clone(), code(3)],
        ] {
            let client = Arc::new(TestSnapClient::new(always(peer, 1, served)));

            let error =
                downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap_err();

            assert_eq!(error, RequestError::BadResponse);
            assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
        }
    }

    #[tokio::test]
    async fn a_response_for_another_request_id_is_rejected() {
        let codes = vec![code(1)];
        let peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new(always(peer, 7, codes.clone())));

        let error = downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(client.reported().len(), usize::from(MAX_RETRIES) + 1);
    }

    #[tokio::test]
    async fn a_wrong_response_type_exhausts_the_retry_budget() {
        let codes = vec![code(1)];
        let peers = [PeerId::random(), PeerId::random(), PeerId::random()];
        let responses = peers.map(|peer| {
            Ok(WithPeerId::new(
                peer,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: Vec::new(),
                    proof: Vec::new(),
                }),
            ))
        });
        let client = Arc::new(TestSnapClient::new(responses));

        let error = downloader(Arc::clone(&client), request(&codes)).unwrap().await.unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(*client.reported(), peers);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High, Priority::High]);
    }

    #[test]
    fn a_request_without_hashes_is_rejected_before_submission() {
        let client = Arc::new(TestSnapClient::new(std::iter::empty()));
        let mut empty = request(&[code(1)]);
        empty.hashes.clear();

        assert_eq!(
            downloader(Arc::clone(&client), empty).unwrap_err(),
            InvalidBytecodeRequest::NoHashes
        );
        assert!(client.priorities().is_empty());
    }
}
