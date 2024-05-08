use super::file_codec::BlockFileCodec;
use futures::Future;
use itertools::Either;
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};
use reth_network_types::PeerId;
use reth_primitives::{
    BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, BytesMut, Header, HeadersDirection,
    SealedHeader, B256,
};
use std::{collections::HashMap, io, path::Path};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::{debug, trace, warn};

/// Default byte length of chunk to read from chain file.
///
/// Default is 1 GB.
pub const DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE: u64 = 1_000_000_000;

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
    #[error("{0}")]
    Rlp(alloy_rlp::Error, Vec<u8>),

    /// Custom error message.
    #[error("{0}")]
    Custom(&'static str),
}

impl From<&'static str> for FileClientError {
    fn from(value: &'static str) -> Self {
        Self::Custom(value)
    }
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

        let mut reader = vec![];
        file.read_to_end(&mut reader).await?;

        Ok(Self::from_reader(&reader[..], file_len).await?.0)
    }

    /// Get the tip hash of the chain.
    pub fn tip(&self) -> Option<B256> {
        self.headers.get(&self.max_block()?).map(|h| h.hash_slow())
    }

    /// Get the start hash of the chain.
    pub fn start(&self) -> Option<B256> {
        self.headers.get(&self.min_block()?).map(|h| h.hash_slow())
    }

    /// Returns the highest block number of this client has or `None` if empty
    pub fn max_block(&self) -> Option<u64> {
        self.headers.keys().max().copied()
    }

    /// Returns the lowest block number of this client has or `None` if empty
    pub fn min_block(&self) -> Option<u64> {
        self.headers.keys().min().copied()
    }

    /// Clones and returns the highest header of this client has or `None` if empty. Seals header
    /// before returning.
    pub fn tip_header(&self) -> Option<SealedHeader> {
        self.headers.get(&self.max_block()?).map(|h| h.clone().seal_slow())
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
    pub fn with_bodies(mut self, bodies: HashMap<BlockHash, BlockBody>) -> Self {
        self.bodies = bodies;
        self
    }

    /// Use the provided headers as the file client's block body buffer.
    pub fn with_headers(mut self, headers: HashMap<BlockNumber, Header>) -> Self {
        self.headers = headers;
        for (number, header) in &self.headers {
            self.hash_to_number.insert(header.hash_slow(), *number);
        }
        self
    }

    /// Returns the current number of headers in the client.
    pub fn headers_len(&self) -> usize {
        self.headers.len()
    }

    /// Returns the current number of bodies in the client.
    pub fn bodies_len(&self) -> usize {
        self.bodies.len()
    }

    /// Returns an iterator over headers in the client.
    pub fn headers_iter(&self) -> impl Iterator<Item = &Header> {
        self.headers.values()
    }

    /// Returns a mutable iterator over bodies in the client.
    pub fn bodies_iter_mut(&mut self) -> impl Iterator<Item = (&u64, &mut BlockBody)> {
        let bodies = &mut self.bodies;
        let headers = &self.headers;
        headers.keys().zip(bodies.values_mut())
    }

    /// Returns the current number of transactions in the client.
    pub fn total_transactions(&self) -> usize {
        self.bodies.iter().flat_map(|(_, body)| &body.transactions).count()
    }
}

impl FromReader for FileClient {
    type Error = FileClientError;

    /// Initialize the [`FileClient`] from bytes that have been read from file.
    fn from_reader<B>(
        reader: B,
        num_bytes: u64,
    ) -> impl Future<Output = Result<(Self, Vec<u8>), Self::Error>>
    where
        B: AsyncReadExt + Unpin,
    {
        let mut headers = HashMap::new();
        let mut hash_to_number = HashMap::new();
        let mut bodies = HashMap::new();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream = FramedRead::with_capacity(reader, BlockFileCodec, num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            "init decode stream"
        );

        let mut remaining_bytes = vec![];

        let mut log_interval = 0;
        let mut log_interval_start_block = 0;

        async move {
            while let Some(block_res) = stream.next().await {
                let block = match block_res {
                    Ok(block) => block,
                    Err(FileClientError::Rlp(err, bytes)) => {
                        trace!(target: "downloaders::file",
                            %err,
                            bytes_len=bytes.len(),
                            "partial block returned from decoding chunk"
                        );
                        remaining_bytes = bytes;
                        break
                    }
                    Err(err) => return Err(err),
                };
                let block_number = block.header.number;
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

                if log_interval == 0 {
                    trace!(target: "downloaders::file",
                        block_number,
                        "read first block"
                    );
                    log_interval_start_block = block_number;
                } else if log_interval % 100_000 == 0 {
                    trace!(target: "downloaders::file",
                        blocks=?log_interval_start_block..=block_number,
                        "read blocks from file"
                    );
                    log_interval_start_block = block_number + 1;
                }
                log_interval += 1;
            }

            trace!(target: "downloaders::file", blocks = headers.len(), "Initialized file client");

            Ok((Self { headers, hash_to_number, bodies }, remaining_bytes))
        }
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

/// Chunks file into several [`FileClient`]s.
#[derive(Debug)]
pub struct ChunkedFileReader {
    /// File to read from.
    file: File,
    /// Current file byte length.
    file_byte_len: u64,
    /// Bytes that have been read.
    chunk: Vec<u8>,
    /// Max bytes per chunk.
    chunk_byte_len: u64,
}

impl ChunkedFileReader {
    /// Returns the remaining file length.
    pub fn file_len(&self) -> u64 {
        self.file_byte_len
    }

    /// Opens the file to import from given path. Returns a new instance. If no chunk byte length
    /// is passed, chunks have [`DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE`] (one static file).
    pub async fn new<P: AsRef<Path>>(
        path: P,
        chunk_byte_len: Option<u64>,
    ) -> Result<Self, FileClientError> {
        let file = File::open(path).await?;
        let chunk_byte_len = chunk_byte_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE);

        Self::from_file(file, chunk_byte_len).await
    }

    /// Opens the file to import from given path. Returns a new instance.
    pub async fn from_file(file: File, chunk_byte_len: u64) -> Result<Self, FileClientError> {
        // get file len from metadata before reading
        let metadata = file.metadata().await?;
        let file_byte_len = metadata.len();

        Ok(Self { file, file_byte_len, chunk: vec![], chunk_byte_len })
    }

    /// Calculates the number of bytes to read from the chain file. Returns a tuple of the chunk
    /// length and the remaining file length.
    fn chunk_len(&self) -> u64 {
        let Self { chunk_byte_len, file_byte_len, .. } = *self;
        let file_byte_len = file_byte_len + self.chunk.len() as u64;

        if chunk_byte_len > file_byte_len {
            // last chunk
            file_byte_len
        } else {
            chunk_byte_len
        }
    }

    /// Read next chunk from file. Returns [`FileClient`] containing decoded chunk.
    pub async fn next_chunk<T>(&mut self) -> Result<Option<T>, T::Error>
    where
        T: FromReader,
    {
        if self.file_byte_len == 0 && self.chunk.is_empty() {
            // eof
            return Ok(None)
        }

        let chunk_target_len = self.chunk_len();
        let old_bytes_len = self.chunk.len() as u64;

        // calculate reserved space in chunk
        let new_read_bytes_target_len = chunk_target_len - old_bytes_len;

        // read new bytes from file
        let mut reader = BytesMut::zeroed(new_read_bytes_target_len as usize);

        // actual bytes that have been read
        let new_read_bytes_len = self.file.read_exact(&mut reader).await? as u64;

        // update remaining file length
        self.file_byte_len -= new_read_bytes_len;

        let prev_read_bytes_len = self.chunk.len();

        // read new bytes from file into chunk
        self.chunk.extend_from_slice(&reader[..]);
        let next_chunk_byte_len = self.chunk.len();

        debug!(target: "downloaders::file",
            max_chunk_byte_len=self.chunk_byte_len,
            prev_read_bytes_len,
            new_read_bytes_target_len,
            new_read_bytes_len,
            reader_capacity=reader.capacity(),
            next_chunk_byte_len,
            remaining_file_byte_len=self.file_byte_len,
            "new bytes were read from file"
        );

        // make new file client from chunk
        let (file_client, bytes) =
            T::from_reader(&self.chunk[..], next_chunk_byte_len as u64).await?;

        // save left over bytes
        self.chunk = bytes;

        Ok(Some(file_client))
    }
}

/// Constructs a file client from a reader.
pub trait FromReader {
    /// Error returned by file client type.
    type Error: From<io::Error>;
    /// Returns a file client
    fn from_reader<B>(
        reader: B,
        num_bytes: u64,
    ) -> impl Future<Output = Result<(Self, Vec<u8>), Self::Error>>
    where
        Self: Sized,
        B: AsyncReadExt + Unpin;
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
    use rand::Rng;
    use reth_consensus::test_utils::TestConsensus;
    use reth_interfaces::p2p::{
        bodies::downloader::BodyDownloader,
        headers::downloader::{HeaderDownloader, SyncTarget},
    };
    use reth_provider::test_utils::create_test_provider_factory;
    use std::{mem, sync::Arc};

    #[tokio::test]
    async fn streams_bodies_from_buffer() {
        // Generate some random blocks
        let factory = create_test_provider_factory();
        let (headers, mut bodies) = generate_bodies(0..=19);

        insert_headers(factory.db_ref().db(), &headers);

        // create an empty file
        let file = tempfile::tempfile().unwrap();

        let client =
            Arc::new(FileClient::from_file(file.into()).await.unwrap().with_bodies(bodies.clone()));
        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            factory,
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
        reth_tracing::init_test_tracing();

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
        let factory = create_test_provider_factory();
        let (file, headers, mut bodies) = generate_bodies_file(0..=19).await;

        // now try to read them back
        let client = Arc::new(FileClient::from_file(file).await.unwrap());

        // insert headers in db for the bodies downloader
        insert_headers(factory.db_ref().db(), &headers);

        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            factory,
        );
        downloader.set_download_range(0..=19).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
    }

    #[tokio::test]
    async fn test_chunk_download_headers_from_file() {
        reth_tracing::init_test_tracing();

        // rig

        const MAX_BYTE_SIZE_HEADER: usize = 720;

        // Generate some random blocks
        let (file, headers, bodies) = generate_bodies_file(0..=14).await;
        // now try to read them back in chunks.
        for header in &headers {
            assert_eq!(720, mem::size_of_val(header))
        }

        // calculate min for chunk byte length range
        let mut bodies_sizes = bodies.values().map(|body| body.size()).collect::<Vec<_>>();
        bodies_sizes.sort();
        let max_block_size = MAX_BYTE_SIZE_HEADER + bodies_sizes.last().unwrap();
        let chunk_byte_len = rand::thread_rng().gen_range(max_block_size..=max_block_size + 10_000);

        trace!(target: "downloaders::file::test", chunk_byte_len);

        // init reader
        let mut reader = ChunkedFileReader::from_file(file, chunk_byte_len as u64).await.unwrap();

        let mut downloaded_headers: Vec<SealedHeader> = vec![];

        let mut local_header = headers.first().unwrap().clone();

        // test

        while let Some(client) = reader.next_chunk::<FileClient>().await.unwrap() {
            let sync_target = client.tip_header().unwrap();
            let sync_target_hash = sync_target.hash();

            // construct headers downloader and use first header
            let mut header_downloader = ReverseHeadersDownloaderBuilder::default()
                .build(Arc::clone(&Arc::new(client)), Arc::new(TestConsensus::default()));
            header_downloader.update_local_head(local_header.clone());
            header_downloader.update_sync_target(SyncTarget::Tip(sync_target_hash));

            // get headers first
            let mut downloaded_headers_chunk = header_downloader.next().await.unwrap().unwrap();

            // export new local header to outer scope
            local_header = sync_target;

            // reverse to make sure it's in the right order before comparing
            downloaded_headers_chunk.reverse();
            downloaded_headers.extend_from_slice(&downloaded_headers_chunk);
        }

        // the first header is not included in the response
        assert_eq!(headers[1..], downloaded_headers);
    }
}
