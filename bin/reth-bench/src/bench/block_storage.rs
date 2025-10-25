//! Block storage module for reading blocks from files

use alloy_consensus::{
    transaction::{Either, EthereumTxEnvelope},
    Block, Header, TxEip4844Variant,
};
use alloy_rlp::Decodable;
use eyre::Context;
use op_alloy_consensus::OpTxEnvelope;
use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};
use tracing::{debug, info};

pub(crate) enum ConsensusBlock {
    Ethereum(Block<EthereumTxEnvelope<TxEip4844Variant>, Header>),
    Op(Block<OpTxEnvelope, Header>),
}

impl ConsensusBlock {
    pub(crate) fn header(&self) -> &Header {
        match self {
            Self::Ethereum(block) => &block.header,
            Self::Op(block) => &block.header,
        }
    }

    pub(crate) fn into_either_block(
        self,
    ) -> Block<Either<EthereumTxEnvelope<TxEip4844Variant>, OpTxEnvelope>, Header> {
        match self {
            Self::Ethereum(block) => block.map_transactions(Either::Left),
            Self::Op(block) => block.map_transactions(Either::Right),
        }
    }
}

/// Detect if a block file contains Optimism blocks
///
/// This works by attempting to decode the first block as both Ethereum and Optimism types.
/// OP blocks have additional fields that Ethereum blocks don't have, so we check for those.
pub(crate) fn detect_optimism_from_file(path: &Path) -> eyre::Result<bool> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    // Read length prefix
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;

    // Read block data
    let mut block_bytes = vec![0u8; len];
    reader.read_exact(&mut block_bytes)?;

    // Try decoding as both types
    let op_decode = Block::<OpTxEnvelope, Header>::decode(&mut block_bytes.as_slice());
    let eth_decode =
        Block::<EthereumTxEnvelope<TxEip4844Variant>, Header>::decode(&mut block_bytes.as_slice());

    match (op_decode, eth_decode) {
        (Ok(_), Err(_)) => {
            // Only OP decode succeeded - definitely Optimism
            info!("Detected Optimism chain from block file");
            Ok(true)
        }
        (Err(_), Ok(_)) => {
            // Only Ethereum decode succeeded - definitely Ethereum
            info!("Detected Ethereum chain from block file");
            Ok(false)
        }
        (Ok(_), Ok(_)) => {
            // Need to check whether this can happen in practice
            info!("Block file format is ambiguous, defaulting to Ethereum");
            Ok(false)
        }
        (Err(op_err), Err(eth_err)) => Err(eyre::eyre!(
            "Failed to decode block as either Ethereum or Optimism.\nOP error: {}\nETH error: {}",
            op_err,
            eth_err
        )),
    }
}

/// Block file reader that streams blocks in batches
pub(crate) struct BlockFileReader {
    reader: BufReader<File>,
    batch_size: usize,
    blocks_read: usize,
    is_optimism: bool,
}

impl BlockFileReader {
    /// Create a new block file reader
    pub(crate) fn new(path: &Path, batch_size: usize, is_optimism: bool) -> eyre::Result<Self> {
        let file =
            File::open(path).wrap_err_with(|| format!("Failed to open block file: {:?}", path))?;

        info!("Opened block file: {:?}", path);

        Ok(Self { reader: BufReader::new(file), batch_size, blocks_read: 0, is_optimism })
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

        let block = if self.is_optimism {
            let block = Block::<OpTxEnvelope, Header>::decode(&mut block_bytes.as_slice())?;
            ConsensusBlock::Op(block)
        } else {
            let block = Block::<EthereumTxEnvelope<TxEip4844Variant>, Header>::decode(
                &mut block_bytes.as_slice(),
            )?;
            ConsensusBlock::Ethereum(block)
        };

        // Skip newline separator if present
        let mut newline = [0u8; 1];
        let _ = self.reader.read_exact(&mut newline);

        self.blocks_read += 1;

        Ok(Some(block))
    }
}

impl Iterator for BlockFileReader {
    type Item = eyre::Result<ConsensusBlock>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_next_block().transpose()
    }
}
