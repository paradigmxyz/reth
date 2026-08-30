//! Scripted snap client shared by the range downloader tests.

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

/// Answers account and storage requests from one queue, so retries across request kinds stay
/// observable in submission order.
#[derive(Debug)]
pub(super) struct TestSnapClient {
    responses: Mutex<VecDeque<PeerRequestResult<SnapResponse>>>,
    reported: Mutex<Vec<PeerId>>,
    options: Mutex<Vec<SnapRequestOptions>>,
    // The capable snap peers, when the test models peer selection. Empty means every request is
    // answered from the queue regardless of its exclusions.
    snap_peers: Mutex<Vec<PeerId>>,
}

impl TestSnapClient {
    /// Creates a client that returns `responses` in request order.
    pub(super) fn new(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            reported: Mutex::new(Vec::new()),
            options: Mutex::new(Vec::new()),
            snap_peers: Mutex::new(Vec::new()),
        }
    }

    // Declares `peers` as the only peers that can serve snap, so a request excluding all of them
    // fails with `RequestError::UnsupportedCapability` the way the fetcher would.
    pub(super) fn with_snap_peers(self, peers: impl IntoIterator<Item = PeerId>) -> Self {
        *self.snap_peers.lock().unwrap() = peers.into_iter().collect();
        self
    }

    /// Returns peers reported for invalid messages.
    pub(super) fn reported(&self) -> MutexGuard<'_, Vec<PeerId>> {
        self.reported.lock().unwrap()
    }

    // The options each request was submitted with, in submission order.
    fn options(&self) -> MutexGuard<'_, Vec<SnapRequestOptions>> {
        self.options.lock().unwrap()
    }

    /// Returns request priorities in submission order.
    pub(super) fn priorities(&self) -> Vec<Priority> {
        self.options().iter().map(|options| options.priority).collect()
    }

    // Returns the exclusions each request was submitted with, in submission order.
    pub(super) fn exclusions(&self) -> Vec<Vec<PeerId>> {
        self.options().iter().map(|options| options.excluded_peers.clone()).collect()
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
        let exhausted = {
            let peers = self.snap_peers.lock().unwrap();
            !peers.is_empty() && peers.iter().all(|peer| options.excluded_peers.contains(peer))
        };
        self.options.lock().unwrap().push(options);
        if exhausted {
            return ready(Err(RequestError::UnsupportedCapability))
        }
        ready(self.responses.lock().unwrap().pop_front().expect("test response available"))
    }
}
