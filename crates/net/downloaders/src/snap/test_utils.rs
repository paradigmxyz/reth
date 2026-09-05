//! Scripted Snap client support for downloader tests.
//!
//! One response queue covers every request kind so cross-request retry behavior stays observable.

use futures::future::{ready, Ready};
use reth_eth_wire_types::snap::SnapProtocolMessage;
use reth_network_p2p::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient, SnapRequestOptions, SnapResponse},
};
use reth_network_peers::PeerId;
use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
};

/// A Snap client that answers requests from a scripted response queue.
#[derive(Debug)]
pub struct TestSnapClient {
    responses: Mutex<VecDeque<PeerRequestResult<SnapResponse>>>,
    reported: Mutex<Vec<PeerId>>,
    priorities: Mutex<Vec<Priority>>,
    exclusions: Mutex<Vec<Vec<PeerId>>>,
}

impl TestSnapClient {
    /// Creates a client that returns `responses` in request order.
    pub fn new(responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            reported: Mutex::new(Vec::new()),
            priorities: Mutex::new(Vec::new()),
            exclusions: Mutex::new(Vec::new()),
        }
    }

    /// Returns peers reported for invalid messages.
    pub fn reported(&self) -> MutexGuard<'_, Vec<PeerId>> {
        self.reported.lock().unwrap()
    }

    /// Returns request priorities in submission order.
    pub fn priorities(&self) -> MutexGuard<'_, Vec<Priority>> {
        self.priorities.lock().unwrap()
    }

    /// Returns peer exclusions in submission order.
    pub fn exclusions(&self) -> MutexGuard<'_, Vec<Vec<PeerId>>> {
        self.exclusions.lock().unwrap()
    }

    // An exhausted script behaves like a network without the requested capability.
    fn next(&self, options: SnapRequestOptions) -> Ready<PeerRequestResult<SnapResponse>> {
        self.priorities.lock().unwrap().push(options.priority);
        self.exclusions.lock().unwrap().push(options.excluded_peers);
        ready(
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(RequestError::UnsupportedCapability)),
        )
    }
}

impl DownloadClient for TestSnapClient {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.reported.lock().unwrap().push(peer_id);
    }

    fn num_connected_peers(&self) -> usize {
        1
    }
}

impl SnapClient for TestSnapClient {
    type Output = Ready<PeerRequestResult<SnapResponse>>;

    fn request_snap(
        &self,
        _request: SnapProtocolMessage,
        options: SnapRequestOptions,
    ) -> Self::Output {
        self.next(options)
    }
}
