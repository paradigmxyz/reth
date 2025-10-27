use crate::bench::{context::BenchContext, output::BLOCK_STORAGE_OUTPUT_SUFFIX};
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};

use alloy_consensus::{
    transaction::{Either, EthereumTxEnvelope},
    Block, BlockBody, Header, TxEip4844Variant,
};
use alloy_rlp::{Decodable, Encodable};
use clap::Parser;
use eyre::{Context, OptionExt};
use op_alloy_consensus::OpTxEnvelope;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};
use tracing::{debug, info};

//type alias for the consensus block
type ConsensusBlock = Block<Either<EthereumTxEnvelope<TxEip4844Variant>, OpTxEnvelope>, Header>;

/// File format version for future compatibility
const FILE_FORMAT_VERSION: u8 = 1;

/// Magic bytes to identify the file format
const MAGIC_BYTES: &[u8] = b"RETH";

/// `reth benchmark generate-file` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

/// Block file header
#[derive(Debug)]
pub(crate) struct BlockFileHeader {
    version: u8,
    block_type: BlockType,
    from_block: u64,
    to_block: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum BlockType {
    Ethereum = 0,
    Optimism = 1,
}

impl BlockFileHeader {
    fn new(is_optimism: bool, from_block: u64, to_block: u64) -> Self {
        Self {
            version: FILE_FORMAT_VERSION,
            block_type: if is_optimism { BlockType::Optimism } else { BlockType::Ethereum },
            from_block,
            to_block,
        }
    }

    /// Get the block range from the header
    pub(super) fn block_range(&self) -> (u64, u64) {
        (self.from_block, self.to_block)
    }

    fn write_to(&self, writer: &mut impl Write) -> eyre::Result<()> {
        writer.write_all(MAGIC_BYTES)?;
        writer.write_all(&[self.version])?;
        writer.write_all(&[self.block_type as u8])?;
        writer.write_all(&self.from_block.to_le_bytes())?;
        writer.write_all(&self.to_block.to_le_bytes())?;
        Ok(())
    }

    /// Read header from file (for the decoder)
    fn read_from(reader: &mut impl std::io::Read) -> eyre::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if magic != MAGIC_BYTES {
            return Err(eyre::eyre!("Invalid file format"));
        }

        let mut version = [0u8; 1];
        reader.read_exact(&mut version)?;
        if version[0] != FILE_FORMAT_VERSION {
            return Err(eyre::eyre!("Unsupported file version: {}", version[0]));
        }

        let mut block_type = [0u8; 1];
        reader.read_exact(&mut block_type)?;

        let mut from_block = [0u8; 8];
        reader.read_exact(&mut from_block)?;
        let from_block = u64::from_le_bytes(from_block);

        let mut to_block = [0u8; 8];
        reader.read_exact(&mut to_block)?;
        let to_block = u64::from_le_bytes(to_block);

        Ok(Self {
            version: version[0],
            block_type: match block_type[0] {
                0 => BlockType::Ethereum,
                1 => BlockType::Optimism,
                _ => return Err(eyre::eyre!("Unknown block type: {}", block_type[0])),
            },
            from_block,
            to_block,
        })
    }
}

/// Writer for block storage files
struct BlockFileWriter {
    writer: BufWriter<File>,
    blocks_written: usize,
}

impl BlockFileWriter {
    fn new(path: &Path, header: BlockFileHeader) -> eyre::Result<Self> {
        let mut writer = BufWriter::new(File::create(path)?);
        header.write_to(&mut writer)?;

        Ok(Self { writer, blocks_written: 0 })
    }

    fn write_block(&mut self, rlp_data: &[u8]) -> eyre::Result<()> {
        // Write length as 4 bytes (supports blocks up to 4GB)
        self.writer.write_all(&(rlp_data.len() as u32).to_le_bytes())?;
        self.writer.write_all(rlp_data)?;
        self.blocks_written += 1;
        Ok(())
    }

    fn finish(mut self) -> eyre::Result<usize> {
        self.writer.flush()?;
        Ok(self.blocks_written)
    }
}

/// Block file reader that streams blocks in batches
pub(crate) struct BlockFileReader {
    reader: BufReader<File>,
    batch_size: usize,
    blocks_read: usize,
    block_type: BlockType,
}

impl BlockFileReader {
    /// Create a new block file reader
    pub(crate) fn new(path: &Path, batch_size: usize) -> eyre::Result<Self> {
        let file =
            File::open(path).wrap_err_with(|| format!("Failed to open block file: {:?}", path))?;

        let mut reader = BufReader::new(file);

        info!("Opened block file: {:?}", path);

        let header = BlockFileHeader::read_from(&mut reader)?;

        let block_type = header.block_type;

        Ok(Self { reader, batch_size, blocks_read: 0, block_type })
    }

    /// Get the header from the file
    pub(crate) fn get_header(path: &Path) -> eyre::Result<BlockFileHeader> {
        let mut file =
            File::open(path).wrap_err_with(|| format!("Failed to open block file: {:?}", path))?;
        BlockFileHeader::read_from(&mut file)
    }

    /// Read the next batch of blocks from the file
    pub(crate) fn read_batch(&mut self) -> eyre::Result<Vec<ConsensusBlock>> {
        let mut batch = Vec::with_capacity(self.batch_size);

        for _ in 0..self.batch_size {
            match self.read_next_block()? {
                Some(block) => batch.push(block),
                None => break, // EOF
            }
        }

        if !batch.is_empty() {
            debug!("Read batch of {} blocks (total read: {})", batch.len(), self.blocks_read);
        }

        Ok(batch)
    }

    /// Read a single block from the file
    fn read_next_block(&mut self) -> eyre::Result<Option<ConsensusBlock>> {
        // Read length prefix (4 bytes, little-endian)
        let mut len_bytes = [0u8; 4];
        match self.reader.read_exact(&mut len_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }

        let len = u32::from_le_bytes(len_bytes) as usize;

        // Read RLP-encoded block data
        let mut block_bytes = vec![0u8; len];
        self.reader.read_exact(&mut block_bytes).wrap_err("Failed to read block data")?;

        let block: ConsensusBlock = match self.block_type {
            BlockType::Ethereum => Block::<EthereumTxEnvelope<TxEip4844Variant>, Header>::decode(
                &mut block_bytes.as_slice(),
            )?
            .map_transactions(|tx| Either::Left(tx)),
            BlockType::Optimism => {
                Block::<OpTxEnvelope, Header>::decode(&mut block_bytes.as_slice())?
                    .map_transactions(|tx| Either::Right(tx))
            }
        };

        self.blocks_read += 1;

        Ok(Some(block))
    }
}

impl Command {
    /// Execute `benchmark block-storage` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Generating file from RPC: {}", self.rpc_url);

        let BenchContext { block_provider, benchmark_mode, mut next_block, is_optimism, .. } =
            BenchContext::new(&self.benchmark, self.rpc_url.clone()).await?;

        // Initialize file writer with header
        let output_path = self
            .benchmark
            .output
            .as_ref()
            .ok_or_eyre("--output is required")?
            .join(BLOCK_STORAGE_OUTPUT_SUFFIX);

        let from_block = self.benchmark.from.ok_or_eyre("--from is required")?;
        let to_block = self.benchmark.to.ok_or_eyre("--to is required")?;

        let header = BlockFileHeader::new(is_optimism, from_block, to_block);
        let mut file_writer = BlockFileWriter::new(&output_path, header)?;

        info!(
            "Writing {} blocks to {:?}",
            if is_optimism { "Optimism" } else { "Ethereum" },
            output_path
        );

        // Process blocks
        while benchmark_mode.contains(next_block) {
            info!("Processing block {}", next_block);

            // Fetch and encode block
            let rlp_data =
                self.fetch_and_encode_block(&block_provider, next_block, is_optimism).await?;

            // Write to file
            file_writer.write_block(&rlp_data)?;

            next_block += 1;
        }

        let blocks_written = file_writer.finish()?;
        info!("Successfully wrote {} blocks to file", blocks_written);

        Ok(())
    }

    async fn fetch_and_encode_block(
        &self,
        provider: &RootProvider<AnyNetwork>,
        block_number: u64,
        is_optimism: bool,
    ) -> eyre::Result<Vec<u8>> {
        // Fetch block
        let block = provider
            .get_block_by_number(block_number.into())
            .full()
            .await?
            .ok_or_eyre(format!("Block {} not found", block_number))?;

        let inner_block = block.into_inner();

        let rlp = if is_optimism {
            // Create Optimism block
            let op_block = Block {
                header: inner_block.header.inner.into_header_with_defaults(),
                body: BlockBody {
                    transactions: inner_block
                        .transactions
                        .into_transactions()
                        .into_iter()
                        .map(|tx| tx.try_into())
                        .collect::<Result<Vec<OpTxEnvelope>, _>>()
                        .map_err(|e| eyre::eyre!("Failed to convert to Optimism tx: {:?}", e))?,
                    ommers: vec![],
                    withdrawals: inner_block.withdrawals,
                },
            };

            let mut buf = Vec::with_capacity(op_block.length());
            op_block.encode(&mut buf);
            buf
        } else {
            // Create Ethereum block
            let eth_block = Block {
                header: inner_block.header.inner.into_header_with_defaults(),
                body: BlockBody {
                    transactions: inner_block
                        .transactions
                        .into_transactions()
                        .into_iter()
                        .map(|tx| tx.try_into_envelope())
                        .collect::<Result<Vec<EthereumTxEnvelope<TxEip4844Variant>>, _>>()
                        .map_err(|e| eyre::eyre!("Failed to convert to Ethereum tx: {:?}", e))?,
                    ommers: vec![],
                    withdrawals: inner_block.withdrawals,
                },
            };

            let mut buf = Vec::with_capacity(eth_block.length());
            eth_block.encode(&mut buf);
            buf
        };

        Ok(rlp)
    }
}
