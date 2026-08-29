//! Scripted snap client shared by the range downloader tests.

use futures::future::{ready, Ready};
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
    GetStorageRangesMessage,
};
use reth_network_p2p::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
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
    priorities: Mutex<Vec<Priority>>,
}

impl TestSnapClient {
    /// Creates a client that returns `responses` in request order.
    pub(super) fn new(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            reported: Mutex::new(Vec::new()),
            priorities: Mutex::new(Vec::new()),
        }
    }

    /// Returns peers reported for invalid messages.
    pub(super) fn reported(&self) -> MutexGuard<'_, Vec<PeerId>> {
        self.reported.lock().unwrap()
    }

    /// Returns request priorities in submission order.
    pub(super) fn priorities(&self) -> MutexGuard<'_, Vec<Priority>> {
        self.priorities.lock().unwrap()
    }

    fn next(&self, priority: Priority) -> Ready<PeerRequestResult<SnapResponse>> {
        self.priorities.lock().unwrap().push(priority);
        ready(self.responses.lock().unwrap().pop_front().expect("test response available"))
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

    fn get_account_range_with_priority(
        &self,
        _request: GetAccountRangeMessage,
        priority: Priority,
    ) -> Self::Output {
        self.next(priority)
    }

    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    fn get_storage_ranges_with_priority(
        &self,
        _request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.next(priority)
    }

    fn get_byte_codes(&self, _request: GetByteCodesMessage) -> Self::Output {
        unsupported()
    }

    fn get_byte_codes_with_priority(
        &self,
        _request: GetByteCodesMessage,
        _priority: Priority,
    ) -> Self::Output {
        unsupported()
    }

    fn get_block_access_lists_with_priority(
        &self,
        _request: GetBlockAccessListsMessage,
        _priority: Priority,
    ) -> Self::Output {
        unsupported()
    }
}

fn unsupported() -> Ready<PeerRequestResult<SnapResponse>> {
    ready(Err(RequestError::UnsupportedCapability))
}
