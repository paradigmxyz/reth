//! Observes and pauses snap requests without replacing the serving peer or response validation.

use crate::wait::WAIT_TIMEOUT;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
    GetStorageRangesMessage, SnapProtocolMessage,
};
use reth_network_p2p::{
    download::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::PeerId;
use reth_node_builder::sync::{BackfillContext, BackfillSyncBuilder};
use reth_node_ethereum::snap::{EthereumBackfill, EthereumBackfillSync};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// Controls the requests of a node's production snap backfill, including across restarts.
#[derive(Clone, Debug, Default)]
pub struct SnapControl {
    state: Arc<Mutex<ControlState>>,
}

impl SnapControl {
    /// Limits account and storage responses to force real peer pagination.
    pub fn with_response_bytes(self, bytes: u64) -> Self {
        self.state.lock().unwrap().response_bytes = Some(bytes);
        self
    }

    /// Pauses the next matching request before it reaches the network.
    ///
    /// Register before starting sync. The gate fires once; cancelling its request does not
    /// pause the retry. Keep the gate alive until releasing it or stopping the node.
    pub fn pause_on(
        &self,
        matches: impl Fn(&SnapProtocolMessage) -> bool + Send + Sync + 'static,
    ) -> SnapGate {
        let (reached, receiver) = watch::channel(false);
        let (release, released) = watch::channel(false);
        self.state.lock().unwrap().gates.push(RequestGate {
            matches: Box::new(matches),
            reached,
            released,
        });
        SnapGate { reached: receiver, release }
    }

    /// Returns the requests seen so far, including a request currently paused at a gate.
    pub fn requests(&self) -> Vec<SnapProtocolMessage> {
        self.state.lock().unwrap().requests.clone()
    }

    /// Wraps the backfill chosen by the node configuration.
    pub fn backfill(&self, inner: EthereumBackfill) -> ControlledBackfill {
        ControlledBackfill { inner, control: self.clone() }
    }
}

/// A single request boundary a test can await and release.
#[derive(Debug)]
pub struct SnapGate {
    reached: watch::Receiver<bool>,
    release: watch::Sender<bool>,
}

impl SnapGate {
    /// Waits until the downloader reaches this request, with the harness timeout.
    pub async fn reached(&mut self) -> eyre::Result<()> {
        tokio::time::timeout(WAIT_TIMEOUT, self.reached.wait_for(|reached| *reached)).await??;
        Ok(())
    }

    /// Lets the paused request proceed to the real peer.
    pub fn release(&self) {
        self.release.send_replace(true);
    }
}

/// Builds the production backfill with an observed network client.
#[derive(Debug)]
pub struct ControlledBackfill {
    inner: EthereumBackfill,
    control: SnapControl,
}

impl<N, C> BackfillSyncBuilder<N, C> for ControlledBackfill
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    type Backfill = EthereumBackfillSync<N, ControlledSnapClient<C>>;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        let client = ControlledSnapClient { inner: ctx.client().clone(), control: self.control };
        let factory = ctx.provider_factory().clone();
        let runtime = ctx.runtime().clone();
        self.inner.build(BackfillContext::new(ctx.into_pipeline(), client, factory, runtime))
    }

    fn recover(&mut self, factory: &ProviderFactory<N>) -> eyre::Result<()> {
        <EthereumBackfill as BackfillSyncBuilder<N, C>>::recover(&mut self.inner, factory)
    }
}

/// A real network client with request observation and deterministic gates.
#[derive(Clone, Debug)]
pub struct ControlledSnapClient<C> {
    inner: C,
    control: SnapControl,
}

impl<C: DownloadClient> DownloadClient for ControlledSnapClient<C> {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.inner.report_bad_message(peer_id);
    }

    fn num_connected_peers(&self) -> usize {
        self.inner.num_connected_peers()
    }
}

impl<C: HeadersClient> HeadersClient for ControlledSnapClient<C> {
    type Header = C::Header;
    type Output = C::Output;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> Self::Output {
        self.inner.get_headers_with_priority(request, priority)
    }
}

impl<C: SnapClient + Clone + 'static> SnapClient for ControlledSnapClient<C> {
    type Output = Pin<Box<dyn Future<Output = PeerRequestResult<SnapResponse>> + Send + Sync>>;

    fn get_account_range_with_priority(
        &self,
        request: GetAccountRangeMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetAccountRange(request), priority)
    }

    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetStorageRanges(request), priority)
    }

    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetByteCodes(request), priority)
    }

    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request(SnapProtocolMessage::GetBlockAccessLists(request), priority)
    }
}

impl<C: SnapClient + Clone + 'static> ControlledSnapClient<C> {
    // Record and gate before dispatching; all responses still come from the real network.
    fn request(
        &self,
        mut request: SnapProtocolMessage,
        priority: Priority,
    ) -> <Self as SnapClient>::Output {
        let gate = {
            let mut state = self.control.state.lock().unwrap();
            if let Some(bytes) = state.response_bytes {
                match &mut request {
                    SnapProtocolMessage::GetAccountRange(request) => request.response_bytes = bytes,
                    SnapProtocolMessage::GetStorageRanges(request) => {
                        request.response_bytes = bytes
                    }
                    _ => {}
                }
            }
            let gate = state
                .gates
                .iter()
                .position(|gate| (gate.matches)(&request))
                .map(|index| state.gates.remove(index));
            state.requests.push(request.clone());
            gate
        };
        let client = self.inner.clone();
        Box::pin(async move {
            if let Some(mut gate) = gate {
                gate.reached.send_replace(true);
                let _ = gate.released.wait_for(|released| *released).await;
            }
            match request {
                SnapProtocolMessage::GetAccountRange(request) => {
                    client.get_account_range_with_priority(request, priority).await
                }
                SnapProtocolMessage::GetStorageRanges(request) => {
                    client.get_storage_ranges_with_priority(request, priority).await
                }
                SnapProtocolMessage::GetByteCodes(request) => {
                    client.get_byte_codes_with_priority(request, priority).await
                }
                SnapProtocolMessage::GetBlockAccessLists(request) => {
                    client.get_block_access_lists_with_priority(request, priority).await
                }
                _ => unreachable!("only requests are dispatched"),
            }
        })
    }
}

#[derive(Default)]
struct ControlState {
    response_bytes: Option<u64>,
    requests: Vec<SnapProtocolMessage>,
    gates: Vec<RequestGate>,
}

impl fmt::Debug for ControlState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlState")
            .field("response_bytes", &self.response_bytes)
            .field("requests", &self.requests)
            .field("gates", &self.gates.len())
            .finish()
    }
}

struct RequestGate {
    matches: Box<dyn Fn(&SnapProtocolMessage) -> bool + Send + Sync>,
    reached: watch::Sender<bool>,
    released: watch::Receiver<bool>,
}
