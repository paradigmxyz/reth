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
        error::RequestError,
        priority::Priority,
    },
    sync::{SyncState, SyncStateProvider, SyncStateUpdater},
};
use reth_primitives::{BlockHash, BlockNumber, Header, PeerId, H256};
use thiserror::Error;
use tokio::{fs::File, io::BufReader};
use tracing::warn;

/// Front-end API for fetching chain data from a file.
///
/// Blocks are assumed to be written one after another in a file, as rlp bytes.
///
/// For example, if the file contains 3 blocks, the file is assumed to be encoded as follows:
/// rlp(block1) || rlp(block2) || rlp(block3)
///
/// Blocks are assumed to have populated transactions, so reading headers will also buffer
/// transactions in memory for use in the bodies stage.
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

/// An error that can occur when constructing and using a [`FileClient`](FileClient).
#[derive(Debug, Error)]
pub enum FileClientError {
    /// An error occurred when opening or reading the file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// An error occurred when decoding blocks, headers, or rlp headers from the file.
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
}

impl FileClient {
    /// Create a new file client from a file path.
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, FileClientError> {
        let file = File::open(path).await?;
        FileClient::from_file(file)
    }

    /// Initialize the [`FileClient`](FileClient) with a file directly.
    pub(crate) fn from_file(file: File) -> Result<Self, FileClientError> {
        let reader = BufReader::new(file);

        Ok(Self {
            reader,
            headers: HashMap::new(),
            bodies: HashMap::new(),
            is_syncing: Arc::new(Default::default()),
        })
    }

    /// Use the provided bodies as the file client's block body buffer.
    pub(crate) fn with_bodies(mut self, bodies: HashMap<BlockHash, BlockBody>) -> Self {
        self.bodies = bodies;
        self
    }

    /// Use the provided headers as the file client's block body buffer.
    pub(crate) fn with_headers(mut self, headers: HashMap<BlockNumber, Header>) -> Self {
        self.headers = headers;
        self
    }
}

impl BodiesClient for FileClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<H256>,
        _priority: Priority,
    ) -> Self::Output {
        // this just searches the buffer, and fails if it can't find the block
        let mut bodies = Vec::new();

        // check if any are an error
        // could unwrap here
        for hash in hashes {
            match self.bodies.get(&hash).cloned() {
                Some(body) => bodies.push(body),
                None => return Box::pin(async move { Err(RequestError::BadResponse) }),
            }
        }

        Box::pin(async move { Ok((PeerId::default(), bodies).into()) })
    }
}

impl DownloadClient for FileClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        warn!("Reported a bad message on a file client, the file may be corrupted or invalid");
        // noop
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::{
            bodies::BodiesDownloaderBuilder,
            test_utils::{insert_headers, zip_blocks},
        },
        test_utils::generate_bodies,
    };
    use assert_matches::assert_matches;
    use futures_util::stream::StreamExt;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_interfaces::{p2p::bodies::downloader::BodyDownloader, test_utils::TestConsensus};
    use std::sync::Arc;

    #[tokio::test]
    async fn streams_bodies_from_buffer() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..20);

        insert_headers(&db, &headers);

        // create an empty file
        let file = tempfile::tempfile().unwrap();

        let client =
            Arc::new(FileClient::from_file(file.into()).unwrap().with_bodies(bodies.clone()));
        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        downloader.set_download_range(0..20).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
    }
}
