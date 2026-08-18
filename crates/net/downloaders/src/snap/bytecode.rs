//! Downloads contract bytecode and authenticates each blob by its content hash.
//!
//! Responses are ordered subsequences of their requests because peers may omit unknown hashes or
//! stop at the response soft limit.

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

/// Downloads contract bytecodes and verifies their requested hashes off the async worker.
#[derive(Debug)]
pub struct BytecodeDownloader<C: SnapClient>(VerifyingRequest<C, BytecodeVerifier>);

impl<C: SnapClient> BytecodeDownloader<C> {
    /// Submits a non-empty bytecode request.
    pub fn new(
        client: C,
        request: GetByteCodesMessage,
        runtime: Runtime,
    ) -> Result<Self, EmptyBytecodeRequest> {
        if request.hashes.is_empty() {
            return Err(EmptyBytecodeRequest)
        }
        let verifier = BytecodeVerifier { request: request.clone() };
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
    /// The responder does not have the requested state.
    Unavailable {
        /// Peer that returned the empty response.
        peer_id: PeerId,
    },
    /// Bytecodes authenticated against their requested hashes.
    Verified(VerifiedBytecodes),
}

/// Requested bytecodes after content-hash authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBytecodes {
    /// Request hashes paired with returned code; omitted codes remain `None`.
    pub codes: Vec<(B256, Option<Bytes>)>,
}

/// A bytecode request that cannot retrieve any data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("bytecode request contains no hashes")]
pub struct EmptyBytecodeRequest;

// Retains request order because omitted hashes cannot be reconstructed from the response.
#[derive(Clone, Debug)]
struct BytecodeVerifier {
    // The verifier owns its input while it runs on the blocking pool.
    request: GetByteCodesMessage,
}

impl SnapVerifier for BytecodeVerifier {
    type Request = GetByteCodesMessage;
    type Output = BytecodeOutcome;

    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError> {
        let SnapResponse::ByteCodes(response) = response else {
            debug!(target: "downloaders::snap", "Expected bytecodes response");
            return Err(RequestError::BadResponse)
        };
        if response.request_id != self.request.request_id {
            debug!(
                target: "downloaders::snap",
                expected = self.request.request_id,
                got = response.request_id,
                "Bytecodes response id mismatch"
            );
            return Err(RequestError::BadResponse)
        }
        if response.codes.is_empty() {
            return Ok(BytecodeOutcome::Unavailable { peer_id })
        }

        let mut codes =
            self.request.hashes.into_iter().map(|hash| (hash, None)).collect::<Vec<_>>();
        let mut next = 0;
        for code in response.codes {
            let hash = keccak256(&code);
            let Some(offset) = codes[next..].iter().position(|(requested, _)| *requested == hash)
            else {
                debug!(target: "downloaders::snap", %hash, "Unrequested or unordered bytecode");
                return Err(RequestError::BadResponse)
            };
            next += offset;
            codes[next].1 = Some(code);
            next += 1;
        }

        Ok(BytecodeOutcome::Verified(VerifiedBytecodes { codes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::{request::MAX_RETRIES, test_utils::TestSnapClient};
    use reth_eth_wire_types::snap::{AccountRangeMessage, ByteCodesMessage};
    use reth_network_p2p::{error::PeerRequestResult, priority::Priority};
    use reth_network_peers::WithPeerId;
    use std::sync::Arc;

    fn request(codes: &[Bytes]) -> GetByteCodesMessage {
        GetByteCodesMessage {
            request_id: 1,
            hashes: codes.iter().map(keccak256).collect(),
            response_bytes: 512 * 1024,
        }
    }

    fn response(peer_id: PeerId, codes: Vec<Bytes>) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            peer_id,
            SnapResponse::ByteCodes(ByteCodesMessage { request_id: 1, codes }),
        ))
    }

    fn downloader(
        client: Arc<TestSnapClient>,
        request: GetByteCodesMessage,
    ) -> Result<BytecodeDownloader<Arc<TestSnapClient>>, EmptyBytecodeRequest> {
        BytecodeDownloader::new(client, request, Runtime::test())
    }

    #[tokio::test]
    async fn verifies_ordered_subsequence_and_preserves_gaps() {
        let requested =
            [Bytes::from_static(&[1]), Bytes::from_static(&[2]), Bytes::from_static(&[3])];
        let client = Arc::new(TestSnapClient::new([response(
            PeerId::random(),
            vec![requested[0].clone(), requested[2].clone()],
        )]));

        let outcome = downloader(client, request(&requested)).unwrap().await.unwrap();

        let BytecodeOutcome::Verified(verified) = outcome else { panic!("verified response") };
        assert_eq!(
            verified.codes,
            vec![
                (keccak256(&requested[0]), Some(requested[0].clone())),
                (keccak256(&requested[1]), None),
                (keccak256(&requested[2]), Some(requested[2].clone())),
            ]
        );
    }

    #[tokio::test]
    async fn verifies_empty_contract_code() {
        let requested = [Bytes::new()];
        let client =
            Arc::new(TestSnapClient::new([response(PeerId::random(), vec![Bytes::new()])]));

        let outcome = downloader(client, request(&requested)).unwrap().await.unwrap();

        assert_eq!(
            outcome,
            BytecodeOutcome::Verified(VerifiedBytecodes {
                codes: vec![(keccak256([]), Some(Bytes::new()))]
            })
        );
    }

    #[tokio::test]
    async fn empty_response_is_unavailable_without_penalty() {
        let requested = [Bytes::from_static(&[1])];
        let peer_id = PeerId::random();
        let client = Arc::new(TestSnapClient::new([response(peer_id, Vec::new())]));

        let outcome = downloader(Arc::clone(&client), request(&requested)).unwrap().await.unwrap();

        assert_eq!(outcome, BytecodeOutcome::Unavailable { peer_id });
        assert!(client.reported().is_empty());
    }

    #[tokio::test]
    async fn invalid_code_is_reported_and_retried() {
        let requested = [Bytes::from_static(&[1]), Bytes::from_static(&[2])];
        let bad_peer = PeerId::random();
        let client = Arc::new(TestSnapClient::new([
            response(bad_peer, vec![Bytes::from_static(&[3])]),
            response(PeerId::random(), requested.to_vec()),
        ]));

        let outcome = downloader(Arc::clone(&client), request(&requested)).unwrap().await.unwrap();

        assert!(matches!(outcome, BytecodeOutcome::Verified(_)));
        assert_eq!(*client.reported(), [bad_peer]);
        assert_eq!(*client.priorities(), [Priority::Normal, Priority::High]);
        assert_eq!(*client.exclusions(), [vec![], vec![bad_peer]]);
    }

    #[tokio::test]
    async fn rejects_code_returned_out_of_request_order() {
        let requested = [Bytes::from_static(&[1]), Bytes::from_static(&[2])];
        let peer_id = PeerId::random();
        let attempts = usize::from(MAX_RETRIES) + 1;
        let client = Arc::new(TestSnapClient::new(
            std::iter::repeat_with(|| response(peer_id, requested.iter().rev().cloned().collect()))
                .take(attempts),
        ));

        let error =
            downloader(Arc::clone(&client), request(&requested)).unwrap().await.unwrap_err();

        assert_eq!(error, RequestError::BadResponse);
        assert_eq!(*client.reported(), vec![peer_id; attempts]);
    }

    #[test]
    fn rejects_empty_request() {
        let client = Arc::new(TestSnapClient::new(std::iter::empty()));
        let request = GetByteCodesMessage { request_id: 1, hashes: Vec::new(), response_bytes: 0 };

        assert!(matches!(downloader(client, request), Err(EmptyBytecodeRequest)));
    }

    #[test]
    fn rejects_wrong_response_kind_and_id() {
        let request = request(&[Bytes::from_static(&[1])]);
        let verifier = BytecodeVerifier { request };
        let peer_id = PeerId::random();
        let wrong_kind = SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        });
        let wrong_id = SnapResponse::ByteCodes(ByteCodesMessage {
            request_id: 2,
            codes: vec![Bytes::from_static(&[1])],
        });

        assert_eq!(verifier.clone().verify(peer_id, wrong_kind), Err(RequestError::BadResponse));
        assert_eq!(verifier.verify(peer_id, wrong_id), Err(RequestError::BadResponse));
    }
}
