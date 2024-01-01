use super::file_codec::BlockFileCodec;
use itertools::Either;
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{
    BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Header, HeadersDirection, PeerId, B256,
};
use std::{self, collections::HashMap, path::Path};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::{trace, warn};

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
/// This reads the entire file into memory, so it is not suitable for large files.
#[derive(Debug)]
pub struct FileClient {
    /// The buffered headers retrieved when fetching new bodies.
    headers: HashMap<BlockNumber, Header>,

    /// A mapping between block hash and number.
    hash_to_number: HashMap<BlockHash, BlockNumber>,

    /// The buffered bodies retrieved when fetching new headers.
    bodies: HashMap<BlockHash, BlockBody>,
}

/// An error that can occur when constructing and using a [`FileClient`].
#[derive(Debug, Error)]
pub enum FileClientError {
    /// An error occurred when opening or reading the file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// An error occurred when decoding blocks, headers, or rlp headers from the file.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}

impl FileClient {
    /// Create a new file client from a file path.
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, FileClientError> {
        let file = File::open(path).await?;
        FileClient::from_file(file).await
    }

    /// Initialize the [`FileClient`] with a file directly.
    pub(crate) async fn from_file(mut file: File) -> Result<Self, FileClientError> {
        // get file len from metadata before reading
        let metadata = file.metadata().await?;
        let file_len = metadata.len();

        // read the entire file into memory
        let mut reader = vec![];
        file.read_to_end(&mut reader).await.unwrap();

        let mut headers = HashMap::new();
        let mut hash_to_number = HashMap::new();
        let mut bodies = HashMap::new();

        // use with_capacity to make sure the internal buffer contains the entire file
        let mut stream = FramedRead::with_capacity(&reader[..], BlockFileCodec, file_len as usize);

        while let Some(block_res) = stream.next().await {
            let block = block_res?;
            let block_hash = block.header.hash_slow();

            // add to the internal maps
            headers.insert(block.header.number, block.header.clone());
            hash_to_number.insert(block_hash, block.header.number);
            bodies.insert(
                block_hash,
                BlockBody {
                    transactions: block.body,
                    ommers: block.ommers,
                    withdrawals: block.withdrawals,
                },
            );
        }

        trace!(blocks = headers.len(), "Initialized file client");

        Ok(Self { headers, hash_to_number, bodies })
    }

    /// Get the tip hash of the chain.
    pub fn tip(&self) -> Option<B256> {
        self.headers.get(&(self.headers.len() as u64)).map(|h| h.hash_slow())
    }

    /// Returns the highest block number of this client has or `None` if empty
    pub fn max_block(&self) -> Option<u64> {
        self.headers.keys().max().copied()
    }

    /// Returns true if all blocks are canonical (no gaps)
    pub fn has_canonical_blocks(&self) -> bool {
        if self.headers.is_empty() {
            return true
        }
        let mut nums = self.headers.keys().copied().collect::<Vec<_>>();
        nums.sort_unstable();
        let mut iter = nums.into_iter();
        let mut lowest = iter.next().expect("not empty");
        for next in iter {
            if next != lowest + 1 {
                return false
            }
            lowest = next;
        }
        true
    }

    /// Use the provided bodies as the file client's block body buffer.
    pub(crate) fn with_bodies(mut self, bodies: HashMap<BlockHash, BlockBody>) -> Self {
        self.bodies = bodies;
        self
    }

    /// Use the provided headers as the file client's block body buffer.
    pub(crate) fn with_headers(mut self, headers: HashMap<BlockNumber, Header>) -> Self {
        self.headers = headers;
        for (number, header) in &self.headers {
            self.hash_to_number.insert(header.hash_slow(), *number);
        }
        self
    }
}

impl HeadersClient for FileClient {
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        // this just searches the buffer, and fails if it can't find the header
        let mut headers = Vec::new();
        trace!(target: "downloaders::file", request=?request, "Getting headers");

        let start_num = match request.start {
            BlockHashOrNumber::Hash(hash) => match self.hash_to_number.get(&hash) {
                Some(num) => *num,
                None => {
                    warn!(%hash, "Could not find starting block number for requested header hash");
                    return Box::pin(async move { Err(RequestError::BadResponse) })
                }
            },
            BlockHashOrNumber::Number(num) => num,
        };

        let range = if request.limit == 1 {
            Either::Left(start_num..start_num + 1)
        } else {
            match request.direction {
                HeadersDirection::Rising => Either::Left(start_num..start_num + request.limit),
                HeadersDirection::Falling => {
                    Either::Right((start_num - request.limit + 1..=start_num).rev())
                }
            }
        };

        trace!(target: "downloaders::file", range=?range, "Getting headers with range");

        for block_number in range {
            match self.headers.get(&block_number).cloned() {
                Some(header) => headers.push(header),
                None => {
                    warn!(number=%block_number, "Could not find header");
                    return Box::pin(async move { Err(RequestError::BadResponse) })
                }
            }
        }

        Box::pin(async move { Ok((PeerId::default(), headers).into()) })
    }
}

impl BodiesClient for FileClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::{
            bodies::BodiesDownloaderBuilder,
            test_utils::{insert_headers, zip_blocks},
        },
        headers::{reverse_headers::ReverseHeadersDownloaderBuilder, test_utils::child_header},
        test_utils::{generate_bodies, generate_bodies_file},
    };
    use assert_matches::assert_matches;
    use futures_util::stream::StreamExt;
    use reth_db::test_utils::create_test_rw_db;
    use reth_interfaces::{
        p2p::{
            bodies::downloader::BodyDownloader,
            headers::downloader::{HeaderDownloader, SyncTarget},
        },
        test_utils::TestConsensus,
    };
    use reth_primitives::{SealedHeader, MAINNET};
    use reth_provider::ProviderFactory;
    use std::sync::Arc;

    #[tokio::test]
    async fn streams_bodies_from_buffer() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=19);

        insert_headers(db.db(), &headers);

        // create an empty file
        let file = tempfile::tempfile().unwrap();

        let client =
            Arc::new(FileClient::from_file(file.into()).await.unwrap().with_bodies(bodies.clone()));
        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            ProviderFactory::new(db, MAINNET.clone()),
        );
        downloader.set_download_range(0..=19).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
    }

    #[tokio::test]
    async fn download_headers_at_fork_head() {
        reth_tracing::init_test_tracing();

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        let file = tempfile::tempfile().unwrap();
        let client = Arc::new(FileClient::from_file(file.into()).await.unwrap().with_headers(
            HashMap::from([
                (0u64, p0.clone().unseal()),
                (1, p1.clone().unseal()),
                (2, p2.clone().unseal()),
                (3, p3.clone().unseal()),
            ]),
        ));

        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .stream_batch_size(3)
            .request_limit(3)
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(p3.clone());
        downloader.update_sync_target(SyncTarget::Tip(p0.hash()));

        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, Ok(vec![p0, p1, p2]));
        assert!(downloader.next().await.is_none());
        assert!(downloader.next().await.is_none());
    }

    #[tokio::test]
    async fn test_download_headers_from_file() {
        // Generate some random blocks
        let (file, headers, _) = generate_bodies_file(0..=19).await;

        // now try to read them back
        let client = Arc::new(FileClient::from_file(file).await.unwrap());

        // construct headers downloader and use first header
        let mut header_downloader = ReverseHeadersDownloaderBuilder::default()
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        header_downloader.update_local_head(headers.first().unwrap().clone());
        header_downloader.update_sync_target(SyncTarget::Tip(headers.last().unwrap().hash()));

        // get headers first
        let mut downloaded_headers = header_downloader.next().await.unwrap().unwrap();

        // reverse to make sure it's in the right order before comparing
        downloaded_headers.reverse();

        // the first header is not included in the response
        assert_eq!(downloaded_headers, headers[1..]);
    }

    #[tokio::test]
    async fn test_download_bodies_from_file() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (file, headers, mut bodies) = generate_bodies_file(0..=19).await;

        // now try to read them back
        let client = Arc::new(FileClient::from_file(file).await.unwrap());

        // insert headers in db for the bodies downloader
        insert_headers(db.db(), &headers);

        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            ProviderFactory::new(db, MAINNET.clone()),
        );
        downloader.set_download_range(0..=19).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
    }
}
