//! Observes and pauses snap requests without replacing the serving peer or response validation.

use crate::{wait::poll_until, E2ETestSetupBuilder};
use parking_lot::Mutex;
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
use reth_node_ethereum::{
    snap::{EthereumBackfill, EthereumBackfillSync},
    EthereumNode,
};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use std::{future::Future, mem, pin::Pin, sync::Arc};
use tokio::sync::watch;

/// Controls the requests of a node's production snap backfill, including across restarts.
#[derive(Clone, Debug, Default)]
pub struct SnapControl {
    response_bytes: Option<u64>,
    state: Arc<Mutex<ControlState>>,
}

impl SnapControl {
    /// Creates a control that limits account and storage responses to force real peer pagination.
    pub fn with_response_bytes(bytes: u64) -> Self {
        Self { response_bytes: Some(bytes), state: Default::default() }
    }

    /// Pauses the next matching request before it reaches the network.
    ///
    /// Only requests sent after this call can match. The gate fires once, so a retry of the paused
    /// request is not paused again. Dropping the gate releases the request.
    pub fn pause_on(
        &self,
        matches: impl Fn(&SnapProtocolMessage) -> bool + Send + Sync + 'static,
    ) -> SnapGate {
        let (state, _) = watch::channel(GateState::Waiting);
        self.state
            .lock()
            .gates
            .push(RequestGate { matches: Box::new(matches), state: state.clone() });
        SnapGate { state }
    }

    /// Returns the requests seen since the last call and clears them, including a request
    /// currently paused at a gate.
    pub fn take_requests(&self) -> Vec<SnapProtocolMessage> {
        mem::take(&mut self.state.lock().requests)
    }
}

impl E2ETestSetupBuilder<EthereumNode> {
    /// Wraps the backfill chosen by the node configuration with `control`.
    pub fn with_snap_control(self, control: &SnapControl) -> Self {
        let control = control.clone();
        self.with_backfill(move |config| ControlledBackfill {
            inner: EthereumBackfill::new(config),
            control: control.clone(),
        })
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
        mut request: GetAccountRangeMessage,
        priority: Priority,
    ) -> Self::Output {
        if let Some(bytes) = self.control.response_bytes {
            request.response_bytes = bytes;
        }
        self.gated(SnapProtocolMessage::GetAccountRange(request.clone()), move |client| {
            client.get_account_range_with_priority(request, priority)
        })
    }

    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    fn get_storage_ranges_with_priority(
        &self,
        mut request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output {
        if let Some(bytes) = self.control.response_bytes {
            request.response_bytes = bytes;
        }
        self.gated(SnapProtocolMessage::GetStorageRanges(request.clone()), move |client| {
            client.get_storage_ranges_with_priority(request, priority)
        })
    }

    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.gated(SnapProtocolMessage::GetByteCodes(request.clone()), move |client| {
            client.get_byte_codes_with_priority(request, priority)
        })
    }

    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        priority: Priority,
    ) -> Self::Output {
        self.gated(SnapProtocolMessage::GetBlockAccessLists(request.clone()), move |client| {
            client.get_block_access_lists_with_priority(request, priority)
        })
    }
}

impl<C: SnapClient + Clone + 'static> ControlledSnapClient<C> {
    // Record and gate before dispatching; all responses still come from the real network.
    fn gated(
        &self,
        request: SnapProtocolMessage,
        send: impl FnOnce(&C) -> C::Output + Send + Sync + 'static,
    ) -> <Self as SnapClient>::Output {
        let released = self.control.state.lock().observe(request);
        let client = self.inner.clone();
        Box::pin(async move {
            if let Some(mut released) = released {
                let _ = released.wait_for(|state| matches!(state, GateState::Released)).await;
            }
            send(&client).await
        })
    }
}

/// A single request boundary a test can await and release.
#[derive(Debug)]
#[must_use = "dropping the gate releases the paused request"]
pub struct SnapGate {
    state: watch::Sender<GateState>,
}

impl SnapGate {
    /// Waits until the downloader reaches this request, with the harness timeout, and returns it.
    pub async fn reached(&self) -> eyre::Result<SnapProtocolMessage> {
        poll_until("a paused snap request", || async {
            Ok(match &*self.state.borrow() {
                GateState::Reached(request) => Some(request.clone()),
                GateState::Waiting | GateState::Released => None,
            })
        })
        .await
    }

    /// Lets the paused request proceed to the real peer.
    pub fn release(&self) {
        self.state.send_replace(GateState::Released);
    }
}

#[derive(Debug, Default)]
struct ControlState {
    requests: Vec<SnapProtocolMessage>,
    gates: Vec<RequestGate>,
}

impl ControlState {
    // Signals a matching gate right away: the request future may be dropped before it is polled.
    fn observe(&mut self, request: SnapProtocolMessage) -> Option<watch::Receiver<GateState>> {
        let released = self.gates.iter().position(|gate| (gate.matches)(&request)).map(|index| {
            let gate = self.gates.remove(index);
            gate.state.send_replace(GateState::Reached(request.clone()));
            gate.state.subscribe()
        });
        self.requests.push(request);
        released
    }
}

#[derive(derive_more::Debug)]
struct RequestGate {
    #[debug(skip)]
    matches: Box<dyn Fn(&SnapProtocolMessage) -> bool + Send + Sync>,
    state: watch::Sender<GateState>,
}

#[derive(Debug)]
enum GateState {
    Waiting,
    Reached(SnapProtocolMessage),
    Released,
}
