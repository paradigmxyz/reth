//! Shared request execution for authenticated snap responses.
//!
//! Retries, peer attribution and blocking verification live here so every range downloader
//! penalizes and reissues in exactly the same way.

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

/// Drives one snap request until its response is verified or retries are exhausted.
pub(super) struct VerifyingRequest<C: SnapClient, V: SnapVerifier> {
    // Sends each attempt and receives the penalty for an invalid response.
    client: C,
    // Verification runs here so peer-controlled proof work stays off the async worker.
    runtime: Runtime,
    // Retained so a retry reissues the identical request.
    request: V::Request,
    // Cloned per attempt, because verification moves onto the blocking pool.
    verifier: V,
    // The response currently in flight.
    fut: C::Output,
    // Present only while a response is being authenticated.
    verification: Option<VerificationTask<V::Output>>,
    // Peers unavailable to this request, including responders that failed verification.
    excluded_peers: Vec<PeerId>,
    // Preserves peer fault attribution if retries exhaust the remaining peers.
    last_verification_error: Option<RequestError>,
    // Attempts already spent against `MAX_RETRIES`.
    retries: u8,
}

impl<C, V> VerifyingRequest<C, V>
where
    C: SnapClient,
    V: SnapVerifier,
{
    /// Submits `request` with custom peer selection options.
    pub(super) fn new_with_options(
        client: C,
        request: V::Request,
        verifier: V,
        runtime: Runtime,
        options: SnapRequestOptions,
    ) -> Self {
        let excluded_peers = options.excluded_peers.clone();
        let fut = request.send(&client, options);
        Self {
            client,
            runtime,
            request,
            verifier,
            fut,
            verification: None,
            excluded_peers,
            last_verification_error: None,
            retries: 0,
        }
    }

    /// Polls until the request yields a verified response or a terminal error.
    ///
    /// An active verification finishes before another response is accepted, so a peer stays
    /// attributable for the work done on its behalf.
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
                // A wrong wire response is already penalized by the session.
                Err(error) if error.is_retryable() || error == RequestError::BadResponse => {
                    debug!(target: "downloaders::snap", %error, "Snap request failed, retrying");
                    if !self.retry() {
                        return Poll::Ready(Err(error))
                    }
                }
                Err(RequestError::UnsupportedCapability) => {
                    return Poll::Ready(Err(self
                        .last_verification_error
                        .clone()
                        .unwrap_or(RequestError::UnsupportedCapability)))
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }
    }

    // Raise retry priority so transient failures cannot leave range progress behind new work.
    fn retry(&mut self) -> bool {
        if self.retries >= MAX_RETRIES {
            return false
        }
        self.retries += 1;
        let options = SnapRequestOptions::new(Priority::High)
            .with_excluded_peers(self.excluded_peers.iter().copied());
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
                self.last_verification_error = Some(error.clone());
                if !self.excluded_peers.contains(&peer_id) {
                    self.excluded_peers.push(peer_id);
                }
                Poll::Ready(self.retry().then_some(None).ok_or(error))
            }
            // A panic or a shutting-down runtime is local, so it must not penalize the responder.
            Err(error) => {
                debug!(target: "downloaders::snap", %error, "Snap verification task failed");
                Poll::Ready(Err(RequestError::ChannelClosed))
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
            .field("last_verification_error", &self.last_verification_error)
            .field("retries", &self.retries)
            .finish_non_exhaustive()
    }
}

/// A snap request that can be reissued at a chosen priority.
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

/// Authenticates a snap response against the request that asked for it.
pub(super) trait SnapVerifier: Clone + Send + 'static {
    /// Request type this verifier authenticates responses for.
    type Request: SnapRequest;
    /// Verified output returned to the downloader.
    type Output: Send + 'static;

    /// Verifies `response`, retaining the responder for non-error outcomes.
    fn verify(self, peer_id: PeerId, response: SnapResponse) -> Result<Self::Output, RequestError>;
}

// Proof failures remain attributable after leaving the async worker.
struct VerificationTask<O> {
    // The responder to penalize if verification rejects its response.
    peer_id: PeerId,
    // Returns the verified output without blocking the async worker.
    fut: tokio::task::JoinHandle<Result<O, RequestError>>,
}
