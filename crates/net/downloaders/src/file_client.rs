use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockHash, BlockNumber, Sealable, B256};
use async_compression::tokio::bufread::GzipDecoder;
use futures::Future;
use itertools::Either;
use reth_consensus::{Consensus, ConsensusError};
use reth_network_p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersDirection, HeadersFut, HeadersRequest},
    priority::Priority,
    BlockClient,
};
use reth_network_peers::PeerId;
use reth_primitives_traits::{Block, BlockBody, FullBlock, SealedBlock, SealedHeader};
use std::{collections::HashMap, io, ops::RangeInclusive, path::Path, sync::Arc};
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncReadExt, BufReader},
};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::{debug, trace, warn};

use super::file_codec::BlockFileCodec;
use crate::receipt_file_client::FromReceiptReader;

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
#[derive(Debug, Clone)]
pub struct FileClient<B: Block> {
    /// The buffered headers retrieved when fetching new bodies.
    headers: HashMap<BlockNumber, B::Header>,

    /// A mapping between block hash and number.
    hash_to_number: HashMap<BlockHash, BlockNumber>,

    /// The buffered bodies retrieved when fetching new headers.
    bodies: HashMap<BlockHash, B::Body>,
}

/// An error that can occur when constructing and using a [`FileClient`].
#[derive(Debug, Error)]
pub enum FileClientError {
    /// An error occurred when validating a header from file.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),

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

impl<B: FullBlock> FileClient<B> {
    /// Create a new file client from a file path.
    pub async fn new<P: AsRef<Path>>(
        path: P,
        consensus: Arc<dyn Consensus<B, Error = ConsensusError>>,
    ) -> Result<Self, FileClientError> {
        let file = File::open(path).await?;
        Self::from_file(file, consensus).await
    }

    /// Initialize the [`FileClient`] with a file directly.
    pub(crate) async fn from_file(
        mut file: File,
        consensus: Arc<dyn Consensus<B, Error = ConsensusError>>,
    ) -> Result<Self, FileClientError> {
        // get file len from metadata before reading
        let metadata = file.metadata().await?;
        let file_len = metadata.len();

        let mut reader = vec![];
        file.read_to_end(&mut reader).await?;

        Ok(FileClientBuilder { consensus, parent_header: None }
            .build(&reader[..], file_len)
            .await?
            .file_client)
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
    pub fn tip_header(&self) -> Option<SealedHeader<B::Header>> {
        self.headers.get(&self.max_block()?).map(|h| SealedHeader::seal_slow(h.clone()))
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
    pub fn with_bodies(mut self, bodies: HashMap<BlockHash, B::Body>) -> Self {
        self.bodies = bodies;
        self
    }

    /// Use the provided headers as the file client's block body buffer.
    pub fn with_headers(mut self, headers: HashMap<BlockNumber, B::Header>) -> Self {
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
    pub fn headers_iter(&self) -> impl Iterator<Item = &B::Header> {
        self.headers.values()
    }

    /// Returns a mutable iterator over bodies in the client.
    ///
    /// Panics, if file client headers and bodies are not mapping 1-1.
    pub fn bodies_iter_mut(&mut self) -> impl Iterator<Item = (u64, &mut B::Body)> {
        let bodies = &mut self.bodies;
        let numbers = &self.hash_to_number;
        bodies.iter_mut().map(|(hash, body)| (numbers[hash], body))
    }

    /// Returns the current number of transactions in the client.
    pub fn total_transactions(&self) -> usize {
        self.bodies.iter().fold(0, |acc, (_, body)| acc + body.transactions().len())
    }
}

struct FileClientBuilder<B: Block> {
    pub consensus: Arc<dyn Consensus<B, Error = ConsensusError>>,
    pub parent_header: Option<SealedHeader<B::Header>>,
}

impl<B: FullBlock<Header: reth_primitives_traits::BlockHeader>> FromReader
    for FileClientBuilder<B>
{
    type Error = FileClientError;
    type Output = FileClient<B>;

    /// Initialize the [`FileClient`] from bytes that have been read from file.
    fn build<R>(
        &self,
        reader: R,
        num_bytes: u64,
    ) -> impl Future<Output = Result<DecodedFileChunk<Self::Output>, Self::Error>>
    where
        R: AsyncReadExt + Unpin,
    {
        let mut headers = HashMap::default();
        let mut hash_to_number = HashMap::default();
        let mut bodies = HashMap::default();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream =
            FramedRead::with_capacity(reader, BlockFileCodec::<B>::default(), num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            "init decode stream"
        );

        let mut remaining_bytes = vec![];

        let mut log_interval = 0;
        let mut log_interval_start_block = 0;

        let mut parent_header = self.parent_header.clone();

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

                let block = SealedBlock::seal_slow(block);

                // Validate standalone header
                self.consensus.validate_header(block.sealed_header())?;
                if let Some(parent) = &parent_header {
                    self.consensus.validate_header_against_parent(block.sealed_header(), parent)?;
                    parent_header = Some(block.sealed_header().clone());
                }

                // Validate block against header
                self.consensus.validate_block_pre_execution(&block)?;

                // add to the internal maps
                let block_hash = block.hash();
                let block_number = block.number();
                let (header, body) = block.split_sealed_header_body();
                headers.insert(block_number, header.unseal());
                hash_to_number.insert(block_hash, block_number);
                bodies.insert(block_hash, body);

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

            Ok(DecodedFileChunk {
                file_client: FileClient { headers, hash_to_number, bodies },
                remaining_bytes,
                highest_block: None,
            })
        }
    }
}

impl<B: FullBlock> HeadersClient for FileClient<B> {
    type Header = B::Header;
    type Output = HeadersFut<B::Header>;

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

impl<B: FullBlock> BodiesClient for FileClient<B> {
    type Body = B::Body;
    type Output = BodiesFut<B::Body>;

    fn get_block_bodies_with_priority_and_range_hint(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
        _range_hint: Option<RangeInclusive<u64>>,
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

impl<B: FullBlock> DownloadClient for FileClient<B> {
    fn report_bad_message(&self, _peer_id: PeerId) {
        trace!("Reported a bad message on a file client, the file may be corrupted or invalid");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are just using a file
        1
    }
}

impl<B: FullBlock> BlockClient for FileClient<B> {
    type Block = B;
}

/// File reader type for handling different compression formats.
#[derive(Debug)]
enum FileReader {
    /// Regular uncompressed file.
    Plain(File),
    /// Gzip compressed file.
    Gzip(GzipDecoder<BufReader<File>>),
}

impl FileReader {
    /// Read data into the provided buffer.
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), io::Error> {
        match self {
            FileReader::Plain(file) => file.read_exact(buf).await?,
            FileReader::Gzip(decoder) => decoder.read_exact(buf).await?,
        };
        Ok(())
    }

    /// Read some data into the provided buffer, returning the number of bytes read.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        match self {
            FileReader::Plain(file) => file.read(buf).await,
            FileReader::Gzip(decoder) => decoder.read(buf).await,
        }
    }
}

/// Chunks file into several [`FileClient`]s.
#[derive(Debug)]
pub struct ChunkedFileReader {
    /// File reader (either plain or gzip).
    file: FileReader,
    /// Current file byte length (for plain files) or estimated length (for gzip).
    file_byte_len: u64,
    /// Bytes that have been read.
    chunk: Vec<u8>,
    /// Max bytes per chunk.
    chunk_byte_len: u64,
    /// Optionally, tracks highest decoded block number. Needed when decoding data that maps * to 1
    /// with block number
    highest_block: Option<u64>,
    /// Whether the file is gzip compressed.
    is_gzip: bool,
}

impl ChunkedFileReader {
    /// Returns the remaining file length.
    pub const fn file_len(&self) -> u64 {
        self.file_byte_len
    }

    /// Opens the file to import from given path. Returns a new instance. If no chunk byte length
    /// is passed, chunks have [`DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE`] (one static file).
    /// Automatically detects gzip files by extension (.gz, .gzip).
    pub async fn new<P: AsRef<Path>>(
        path: P,
        chunk_byte_len: Option<u64>,
    ) -> Result<Self, FileClientError> {
        let path = path.as_ref();
        let file = File::open(path).await?;
        let chunk_byte_len = chunk_byte_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE);

        Self::from_file(
            file,
            chunk_byte_len,
            path.extension()
                .and_then(|ext| ext.to_str())
                .map_or(false, |ext| ["gz", "gzip"].contains(&ext)),
        )
        .await
    }

    /// Opens the file to import from given path. Returns a new instance.
    pub async fn from_file(
        file: File,
        chunk_byte_len: u64,
        is_gzip: bool,
    ) -> Result<Self, FileClientError> {
        let file_byte_len = if is_gzip {
            // For gzipped files, we can't know the decompressed size without reading the entire
            // file Use a very large value to indicate unknown size
            u64::MAX
        } else {
            // Get file len from metadata before reading
            file.metadata().await?.len()
        };

        let file_reader = if is_gzip {
            FileReader::Gzip(GzipDecoder::new(BufReader::new(file)))
        } else {
            FileReader::Plain(file)
        };

        Ok(Self {
            file: file_reader,
            file_byte_len,
            chunk: vec![],
            chunk_byte_len,
            highest_block: None,
            is_gzip,
        })
    }

    /// Calculates the number of bytes to read from the chain file. Returns a tuple of the chunk
    /// length and the remaining file length.
    const fn chunk_len(&self) -> u64 {
        let Self { chunk_byte_len, file_byte_len, .. } = *self;

        // For gzipped files, we don't know the decompressed size, so always use chunk_byte_len
        if self.is_gzip {
            return chunk_byte_len;
        }

        let file_byte_len = file_byte_len + self.chunk.len() as u64;

        if chunk_byte_len > file_byte_len {
            // last chunk
            file_byte_len
        } else {
            chunk_byte_len
        }
    }

    /// Reads bytes from file and buffers as next chunk to decode. Returns byte length of next
    /// chunk to read.
    async fn read_next_chunk(&mut self) -> Result<Option<u64>, io::Error> {
        if !self.is_gzip && self.file_byte_len == 0 && self.chunk.is_empty() {
            // eof for non-gzipped files
            return Ok(None)
        }

        let chunk_target_len = self.chunk_len();
        let old_bytes_len = self.chunk.len() as u64;

        // If we already have enough data in the chunk, return it
        if old_bytes_len >= chunk_target_len {
            return Ok(Some(old_bytes_len))
        }

        // For gzipped files, if we have no data in chunk, try to read some to check EOF
        if self.is_gzip && self.chunk.is_empty() {
            // Try to read a small amount to check if we're at EOF
            let mut buf = vec![0u8; 128];
            match self.file.read(&mut buf).await {
                Ok(0) => return Ok(None), // EOF
                Ok(n) => {
                    // We read some bytes, put them in the chunk
                    self.chunk.extend_from_slice(&buf[..n]);
                    // If we have enough data now, return it
                    if self.chunk.len() as u64 >= chunk_target_len {
                        return Ok(Some(chunk_target_len))
                    }
                    // Otherwise continue to read more
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e),
            }
        }

        // calculate reserved space in chunk
        let new_read_bytes_target_len = chunk_target_len - self.chunk.len() as u64;

        // read new bytes from file
        let prev_read_bytes_len = self.chunk.len();
        self.chunk.extend(std::iter::repeat_n(0, new_read_bytes_target_len as usize));
        let reader = &mut self.chunk[prev_read_bytes_len..];

        // For gzipped files, read_exact might fail at EOF, so we handle it gracefully
        let new_read_bytes_len = match self.file.read_exact(reader).await {
            Ok(()) => {
                // update remaining file length (only for non-gzipped files)
                if !self.is_gzip {
                    self.file_byte_len -= new_read_bytes_target_len;
                }
                new_read_bytes_target_len
            }
            Err(e) if self.is_gzip && e.kind() == io::ErrorKind::UnexpectedEof => {
                // For gzipped files, this indicates we've reached the end
                // Try to read whatever bytes are available
                let mut actual_read = 0;
                while actual_read < new_read_bytes_target_len as usize {
                    match self.file.read(&mut reader[actual_read..]).await {
                        Ok(0) => break,
                        Ok(n) => actual_read += n,
                        Err(_) => break,
                    }
                }
                self.chunk.truncate(prev_read_bytes_len + actual_read);

                // If no new data was read and we have no data in chunk, we're at EOF
                if actual_read == 0 && self.chunk.is_empty() {
                    return Ok(None)
                }

                actual_read as u64
            }
            Err(e) => return Err(e),
        };

        let next_chunk_byte_len = self.chunk.len();

        debug!(target: "downloaders::file",
            max_chunk_byte_len=self.chunk_byte_len,
            prev_read_bytes_len,
            new_read_bytes_target_len,
            new_read_bytes_len,
            next_chunk_byte_len,
            remaining_file_byte_len=self.file_byte_len,
            "new bytes were read from file"
        );

        Ok(Some(next_chunk_byte_len as u64))
    }

    /// Read next chunk from file. Returns [`FileClient`] containing decoded chunk.
    /// Reads gzipped data until we accumulate at least chunk_byte_len bytes.
    ///
    /// Returns `Ok(true)` if data was successfully read and we have enough bytes,
    /// or `Ok(false)` if EOF was reached with no remaining data.
    async fn read_gzip_chunk(&mut self) -> Result<bool, FileClientError> {
        loop {
            // If we already have enough data, return true
            if self.chunk.len() > self.chunk_byte_len as usize {
                return Ok(true)
            }

            // Read more data from the gzipped stream
            let mut buffer = vec![0u8; 64 * 1024];

            // Note: gzip decoder may not utilize the full buffer size due to internal
            // limitations
            match self.file.read(&mut buffer).await {
                Ok(0) => {
                    // EOF reached - return whether have any remaining data
                    return Ok(!self.chunk.is_empty())
                }
                Ok(n) => {
                    buffer.truncate(n);
                    self.chunk.extend_from_slice(&buffer);
                    // Continue the loop to check if we have enough data now
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Read next chunk from file. Returns [`FileClient`] containing decoded chunk.
    ///
    /// For gzipped files, this method accumulates data until at least `chunk_byte_len` bytes
    /// are available before processing. For plain files, it uses the original chunking logic.
    pub async fn next_chunk<B: FullBlock>(
        &mut self,
        consensus: Arc<dyn Consensus<B, Error = ConsensusError>>,
        parent_header: Option<SealedHeader<B::Header>>,
    ) -> Result<Option<FileClient<B>>, FileClientError> {
        let chunk_len = if self.is_gzip {
            if !self.read_gzip_chunk().await? {
                return Ok(None)
            }
            self.chunk.len() as u64
        } else {
            let Some(chunk_len) = self.read_next_chunk().await? else { return Ok(None) };
            chunk_len
        };

        // make new file client from chunk
        let DecodedFileChunk { file_client, remaining_bytes, .. } =
            FileClientBuilder { consensus, parent_header }
                .build(&self.chunk[..], chunk_len)
                .await?;

        // save left over bytes
        self.chunk = remaining_bytes;

        Ok(Some(file_client))
    }

    /// Read next chunk from file. Returns [`FileClient`] containing decoded chunk.
    pub async fn next_receipts_chunk<T>(&mut self) -> Result<Option<T>, T::Error>
    where
        T: FromReceiptReader,
    {
        let Some(next_chunk_byte_len) = self.read_next_chunk().await? else { return Ok(None) };

        // make new file client from chunk
        let DecodedFileChunk { file_client, remaining_bytes, highest_block } =
            T::from_receipt_reader(&self.chunk[..], next_chunk_byte_len, self.highest_block)
                .await?;

        // save left over bytes
        self.chunk = remaining_bytes;
        // update highest block
        self.highest_block = highest_block;

        Ok(Some(file_client))
    }
}

/// Constructs a file client from a reader.
pub trait FromReader {
    /// Error returned by file client type.
    type Error: From<io::Error>;

    /// Output returned by file client type.
    type Output;

    /// Returns a file client
    fn build<R>(
        &self,
        reader: R,
        num_bytes: u64,
    ) -> impl Future<Output = Result<DecodedFileChunk<Self::Output>, Self::Error>>
    where
        Self: Sized,
        R: AsyncReadExt + Unpin;
}

/// Output from decoding a file chunk with [`FromReader::build`].
#[derive(Debug)]
pub struct DecodedFileChunk<T> {
    /// File client, i.e. the decoded part of chunk.
    pub file_client: T,
    /// Remaining bytes that have not been decoded, e.g. a partial block or a partial receipt.
    pub remaining_bytes: Vec<u8>,
    /// Highest block of decoded chunk. This is needed when decoding data that maps * to 1 with
    /// block number, like receipts.
    pub highest_block: Option<u64>,
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
    use async_compression::tokio::write::GzipEncoder;
    use futures_util::stream::StreamExt;
    use rand::Rng;
    use reth_consensus::{noop::NoopConsensus, test_utils::TestConsensus};
    use reth_ethereum_primitives::Block;
    use reth_network_p2p::{
        bodies::downloader::BodyDownloader,
        headers::downloader::{HeaderDownloader, SyncTarget},
    };
    use reth_provider::test_utils::create_test_provider_factory;
    use std::sync::Arc;
    use tokio::{
        fs::File,
        io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    };

    #[tokio::test]
    async fn streams_bodies_from_buffer() {
        // Generate some random blocks
        let factory = create_test_provider_factory();
        let (headers, mut bodies) = generate_bodies(0..=19);

        insert_headers(factory.db_ref().db(), &headers);

        // create an empty file
        let file = tempfile::tempfile().unwrap();

        let client: Arc<FileClient<Block>> = Arc::new(
            FileClient::from_file(file.into(), NoopConsensus::arc())
                .await
                .unwrap()
                .with_bodies(bodies.clone()),
        );
        let mut downloader = BodiesDownloaderBuilder::default().build::<Block, _, _>(
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
        let client: Arc<FileClient<Block>> = Arc::new(
            FileClient::from_file(file.into(), NoopConsensus::arc()).await.unwrap().with_headers(
                HashMap::from([
                    (0u64, p0.clone_header()),
                    (1, p1.clone_header()),
                    (2, p2.clone_header()),
                    (3, p3.clone_header()),
                ]),
            ),
        );

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
        let client: Arc<FileClient<Block>> =
            Arc::new(FileClient::from_file(file, NoopConsensus::arc()).await.unwrap());

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
        let client: Arc<FileClient<Block>> =
            Arc::new(FileClient::from_file(file, NoopConsensus::arc()).await.unwrap());

        // insert headers in db for the bodies downloader
        insert_headers(factory.db_ref().db(), &headers);

        let mut downloader = BodiesDownloaderBuilder::default().build::<Block, _, _>(
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

        // Generate some random blocks
        let (file, headers, _) = generate_bodies_file(0..=14).await;

        // calculate min for chunk byte length range, pick a lower bound that guarantees at least
        // one block will be read
        let chunk_byte_len = rand::rng().random_range(2000..=10_000);
        trace!(target: "downloaders::file::test", chunk_byte_len);

        // init reader
        let mut reader =
            ChunkedFileReader::from_file(file, chunk_byte_len as u64, false).await.unwrap();

        let mut downloaded_headers: Vec<SealedHeader> = vec![];

        let mut local_header = headers.first().unwrap().clone();

        // test
        while let Some(client) =
            reader.next_chunk::<Block>(NoopConsensus::arc(), None).await.unwrap()
        {
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

    #[tokio::test]
    async fn test_chunk_download_headers_from_gzip_file() {
        reth_tracing::init_test_tracing();

        // Generate some random blocks
        let (file, headers, _) = generate_bodies_file(0..=14).await;

        // Create a gzipped version of the file
        let gzip_temp_file = tempfile::NamedTempFile::new().unwrap();
        let gzip_path = gzip_temp_file.path().to_owned();
        drop(gzip_temp_file); // Close the file so we can write to it

        // Read original file content first
        let mut original_file = file;
        original_file.seek(SeekFrom::Start(0)).await.unwrap();
        let mut original_content = Vec::new();
        original_file.read_to_end(&mut original_content).await.unwrap();

        let mut gzip_file = File::create(&gzip_path).await.unwrap();
        let mut encoder = GzipEncoder::new(&mut gzip_file);

        // Write the original content through the gzip encoder
        encoder.write_all(&original_content).await.unwrap();
        encoder.shutdown().await.unwrap();
        drop(gzip_file);

        // Reopen the gzipped file for reading
        let gzip_file = File::open(&gzip_path).await.unwrap();

        // calculate min for chunk byte length range, pick a lower bound that guarantees at least
        // one block will be read
        let chunk_byte_len = rand::rng().random_range(2000..=10_000);
        trace!(target: "downloaders::file::test", chunk_byte_len);

        // init reader with gzip=true
        let mut reader =
            ChunkedFileReader::from_file(gzip_file, chunk_byte_len as u64, true).await.unwrap();

        let mut downloaded_headers: Vec<SealedHeader> = vec![];

        let mut local_header = headers.first().unwrap().clone();

        // test
        while let Some(client) =
            reader.next_chunk::<Block>(NoopConsensus::arc(), None).await.unwrap()
        {
            if client.headers_len() == 0 {
                continue;
            }

            let sync_target = client.tip_header().expect("tip_header should not be None");

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
