//! Shared request execution for authenticated Snap responses.
//!
//! Proof verification runs off the async worker while peer attribution and retries remain in one
//! place for every range downloader.

use futures::FutureExt;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
    GetStorageRangesMessage, SnapProtocolMessage,
};
use reth_network_p2p::{
    error::RequestError,
    priority::Priority,
    snap::client::{SnapClient, SnapRequestOptions, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_tasks::Runtime;
use std::{
    fmt,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// Number of retries allowed after the initial request fails.
pub(super) const MAX_RETRIES: u8 = 2;

/// Drives one Snap request until its response is verified or retries are exhausted.
pub(super) struct VerifyingRequest<C: SnapClient, V: SnapVerifier> {
    client: C,
    runtime: Runtime,
    request: V::Request,
    verifier: V,
    fut: C::Output,
    verification: Option<VerificationTask<V::Output>>,
    excluded_peers: Vec<PeerId>,
    retries: u8,
}

impl<C, V> VerifyingRequest<C, V>
where
    C: SnapClient,
    V: SnapVerifier,
{
    /// Submits `request` at normal priority and verifies its response with `verifier`.
    pub(super) fn new(client: C, request: V::Request, verifier: V, runtime: Runtime) -> Self {
        let fut = request.send(&client, SnapRequestOptions::default());
        Self {
            client,
            runtime,
            request,
            verifier,
            fut,
            verification: None,
            excluded_peers: Vec::new(),
            retries: 0,
        }
    }

    /// Polls until the request yields a verified response or a terminal error.
    pub(super) fn poll_verified(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<V::Output, RequestError>> {
        loop {
            if self.verification.is_some() {
                match ready!(self.poll_verification(cx)) {
                    Ok(Some(output)) => return Poll::Ready(Ok(output)),
                    Ok(None) => {}
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }

            match ready!(self.fut.poll_unpin(cx)) {
                Ok(response) => {
                    let (peer_id, response) = response.split();
                    let verifier = self.verifier.clone();
                    let fut =
                        self.runtime.spawn_blocking(move || verifier.verify(peer_id, response));
                    self.verification = Some(VerificationTask { peer_id, fut });
                }
                // Wire-level bad responses are already penalized by the session.
                Err(error) if error.is_retryable() || error == RequestError::BadResponse => {
                    debug!(target: "downloaders::snap", %error, "Snap request failed, retrying");
                    if !self.retry() {
                        return Poll::Ready(Err(error))
                    }
                }
                Err(RequestError::UnsupportedCapability) if !self.excluded_peers.is_empty() => {
                    return Poll::Ready(Err(RequestError::BadResponse))
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }

    // High priority keeps retry progress ahead of newly queued range work.
    fn retry(&mut self) -> bool {
        if self.retries >= MAX_RETRIES {
            return false
        }
        self.retries += 1;
        let options = SnapRequestOptions::new(Priority::High)
            .with_excluded_peers(self.excluded_peers.clone());
        self.fut = self.request.send(&self.client, options);
        true
    }

    // The responder stays attached until blocking verification completes.
    fn poll_verification(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<V::Output>, RequestError>> {
        let verification = self.verification.as_mut().expect("verification task is present");
        let result = ready!(verification.fut.poll_unpin(cx));
        let peer_id = verification.peer_id;
        self.verification = None;

        match result {
            Ok(Ok(output)) => Poll::Ready(Ok(Some(output))),
            Ok(Err(error)) => {
                debug!(target: "downloaders::snap", ?peer_id, %error, "Invalid snap response");
                self.client.report_bad_message(peer_id);
                if !self.excluded_peers.contains(&peer_id) {
                    self.excluded_peers.push(peer_id);
                }
                Poll::Ready(self.retry().then_some(None).ok_or(error))
            }
            // Panics and runtime shutdowns are local, so they must not penalize the responder.
            Err(error) => {
                debug!(target: "downloaders::snap", %error, "Snap verification task failed");
                Poll::Ready(Err(RequestError::Internal))
            }
        }
    }
}

// The opaque client future cannot be printed without imposing an unnecessary bound.
impl<C, V> fmt::Debug for VerifyingRequest<C, V>
where
    C: SnapClient,
    V: SnapVerifier + fmt::Debug,
    V::Request: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifyingRequest")
            .field("client", &self.client)
            .field("request", &self.request)
            .field("verifier", &self.verifier)
            .field("verifying", &self.verification.is_some())
            .field("excluded_peers", &self.excluded_peers)
            .field("retries", &self.retries)
            .finish_non_exhaustive()
    }
}

/// A Snap request that can be reissued at a chosen priority.
pub(super) trait SnapRequest {
    /// Sends this request through `client`.
    fn send<C: SnapClient>(&self, client: &C, options: SnapRequestOptions) -> C::Output;
}

impl SnapRequest for GetAccountRangeMessage {
    fn send<C: SnapClient>(&self, client: &C, options: SnapRequestOptions) -> C::Output {
        client.request_snap(SnapProtocolMessage::GetAccountRange(self.clone()), options)
    }
}

impl SnapRequest for GetStorageRangesMessage {
    fn send<C: SnapClient>(&self, client: &C, options: SnapRequestOptions) -> C::Output {
        client.request_snap(SnapProtocolMessage::GetStorageRanges(self.clone()), options)
    }
}

impl SnapRequest for GetByteCodesMessage {
    fn send<C: SnapClient>(&self, client: &C, options: SnapRequestOptions) -> C::Output {
        client.request_snap(SnapProtocolMessage::GetByteCodes(self.clone()), options)
    }
}

impl SnapRequest for GetBlockAccessListsMessage {
    fn send<C: SnapClient>(&self, client: &C, options: SnapRequestOptions) -> C::Output {
        client.request_snap(SnapProtocolMessage::GetBlockAccessLists(self.clone()), options)
    }
}

/// Authenticates a Snap response against its request.
pub(super) trait SnapVerifier: Clone + Send + 'static {
    /// Request type accepted by this verifier.
    type Request: SnapRequest;
    /// Verified output returned to the downloader.
    type Output: Send + 'static;

    /// Verifies `response`, retaining the responder for non-error outcomes.
    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError>;
}

// Proof failures remain attributable after leaving the async worker.
struct VerificationTask<O> {
    peer_id: PeerId,
    fut: tokio::task::JoinHandle<Result<O, RequestError>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::test_utils::TestSnapClient;
    use alloy_primitives::B256;
    use futures::future::poll_fn;
    use reth_eth_wire_types::snap::{AccountRangeMessage, GetAccountRangeMessage};
    use reth_network_peers::WithPeerId;
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    struct PanickingVerifier;

    impl SnapVerifier for PanickingVerifier {
        type Request = GetAccountRangeMessage;
        type Output = ();

        fn verify(
            self,
            _peer_id: PeerId,
            _response: SnapResponse,
        ) -> Result<Self::Output, RequestError> {
            panic!("local verifier panic")
        }
    }

    #[tokio::test]
    async fn verifier_panic_is_internal_without_peer_penalty() {
        let peer_id = PeerId::random();
        let response = SnapResponse::AccountRange(AccountRangeMessage {
            request_id: 1,
            accounts: Vec::new(),
            proof: Vec::new(),
        });
        let client = Arc::new(TestSnapClient::new([Ok(WithPeerId::new(peer_id, response))]));
        let request = GetAccountRangeMessage {
            request_id: 1,
            root_hash: B256::ZERO,
            starting_hash: B256::ZERO,
            limit_hash: B256::ZERO,
            response_bytes: 0,
        };
        let mut verifying =
            VerifyingRequest::new(Arc::clone(&client), request, PanickingVerifier, Runtime::test());

        let error = poll_fn(|cx| verifying.poll_verified(cx)).await.unwrap_err();

        assert_eq!(error, RequestError::Internal);
        assert!(client.reported().is_empty());
    }
}
