use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use reth_eth_wire::BlockBody;
use reth_interfaces::{
    p2p::{
        bodies::client::{BodiesClient, BodiesFut},
        download::DownloadClient,
        headers::client::{HeadersClient, HeadersFut, HeadersRequest},
        priority::Priority,
    },
    sync::{SyncState, SyncStateProvider, SyncStateUpdater},
};
use reth_primitives::{BlockHash, BlockNumber, Header, PeerId, H256};
use tokio::{fs::File, io::BufReader};

/// Front-end API for fetching chain data from a file.
///
/// Blocks are assumed to be written one after another in a file, as rlp bytes.
///
/// For example, if the file contains 3 blocks, the file is assumed to be encoded as follows:
/// rlp(block1) || rlp(block2) || rlp(block3)
///
/// Blocks are assumed to have populated transactions, so reading headers will also buffer
/// transactions in memory for use in the bodies stage.
///
/// Likewise, if a block body is requested and is not buffered, it will be read from the file and
/// the header information will be buffered.
#[derive(Debug)]
pub struct FileClient {
    /// The open reader for the file.
    reader: BufReader<File>,

    /// The buffered headers retrieved when fetching new bodies.
    headers: HashMap<BlockNumber, Header>,

    /// The buffered bodies retrieved when fetching new headers.
    bodies: HashMap<BlockHash, BlockBody>,

    /// Represents if we are currently syncing.
    is_syncing: Arc<AtomicBool>,
}

impl FileClient {
    /// Create a new file client from a file path.
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let file = File::open(path).await?;
        let reader = BufReader::new(file);

        Ok(Self {
            reader,
            headers: HashMap::new(),
            bodies: HashMap::new(),
            is_syncing: Arc::new(Default::default()),
        })
    }
}

impl HeadersClient for FileClient {
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        // first search the buffer

        // then read from the file if not found
        todo!()
    }
}

impl BodiesClient for FileClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        _priority: Priority,
    ) -> Self::Output {
        // first search the buffer

        // then read from the file if not found
        todo!()
    }
}

impl DownloadClient for FileClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // this should never happen? but we should report an error of some sort
        todo!()
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are just using a file
        1
    }
}

impl SyncStateProvider for FileClient {
    fn is_syncing(&self) -> bool {
        self.is_syncing.load(Ordering::Relaxed)
    }
}

impl SyncStateUpdater for FileClient {
    fn update_sync_state(&self, state: SyncState) {
        let is_syncing = state.is_syncing();
        self.is_syncing.store(is_syncing, Ordering::Relaxed)
    }
}
