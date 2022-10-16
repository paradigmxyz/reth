//! Block related types

use crate::info::PieceIndex;
use std::{fmt, ops::Deref, sync::Arc};

/// This is the only block length we're dealing with (except for possibly the
/// last block).  It is the widely used and accepted 16 KiB.
pub(crate) const BLOCK_LEN: u32 = 0x4000;

/// A block is a fixed size chunk of a piece, which in turn is a fixed size
/// chunk of a torrent. Downloading torrents happen at this block level
/// granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BlockInfo {
    /// The index of the piece of which this is a block.
    pub piece_index: PieceIndex,
    /// The zero-based byte offset into the piece.
    pub offset: u32,
    /// The block's length in bytes. Always 16 KiB (0x4000 bytes) or less, for
    /// now.
    pub len: u32,
}

impl BlockInfo {
    /// Returns the index of the block within its piece, assuming the default
    /// block length of 16 KiB.
    pub fn index_in_piece(&self) -> usize {
        // we need to use "lower than or equal" as this may be the last block in
        // which case it may be shorter than the default block length
        debug_assert!(self.len <= BLOCK_LEN);
        debug_assert!(self.len > 0);
        (self.offset / BLOCK_LEN) as usize
    }
}

impl fmt::Display for BlockInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(piece: {} offset: {} len: {})", self.piece_index, self.offset, self.len)
    }
}

/// Returns the length of the block at the index in piece.
///
/// If the piece is not a multiple of the default block length, the returned
/// value is smalled.
///
/// # Panics
///
/// Panics if the index multiplied by the default block length would exceed the
/// piece length.
pub(crate) fn block_len(piece_len: u32, block_index: usize) -> u32 {
    let block_index = block_index as u32;
    let block_offset = block_index * BLOCK_LEN;
    assert!(piece_len > block_offset);
    std::cmp::min(piece_len - block_offset, BLOCK_LEN)
}

/// Returns the number of blocks in a piece of the given length.
pub(crate) fn block_count(piece_len: u32) -> usize {
    // all but the last piece are a multiple of the block length, but the
    // last piece may be shorter so we need to account for this by rounding
    // up before dividing to get the number of blocks in piece
    (piece_len as usize + (BLOCK_LEN as usize - 1)) / BLOCK_LEN as usize
}

/// A piece block that contains the block's metadata and data.
pub(crate) struct Block {
    /// The index of the piece of which this is a block.
    pub piece_index: PieceIndex,
    /// The zero-based byte offset into the piece.
    pub offset: u32,
    /// The actual raw data of the block.
    pub data: BlockData,
}

impl Block {
    /// Constructs a new block based on the metadata and data.
    pub fn new(info: BlockInfo, data: impl Into<BlockData>) -> Self {
        Self { piece_index: info.piece_index, offset: info.offset, data: data.into() }
    }

    /// Returns a [`BlockInfo`] representing the metadata of this block.
    pub fn info(&self) -> BlockInfo {
        BlockInfo {
            piece_index: self.piece_index,
            offset: self.offset,
            len: self.data.len() as u32,
        }
    }
}

/// Abstracts over the block data type.
///
/// A block may be just a normal byte buffer, or it may be a reference into
/// a cache.
#[derive(Debug, PartialEq)]
#[cfg_attr(test, derive(Clone))]
pub(crate) enum BlockData {
    Owned(Vec<u8>),
    Cached(CachedBlock),
}

/// Blocks are cached in memory and are shared between the disk task and peer
/// session tasks. Therefore we use atomic reference counting to make sure that
/// even if a block is evicted from cache, the peer still using it still has
/// a valid reference to it.
pub(crate) type CachedBlock = Arc<Vec<u8>>;

impl BlockData {
    /// Returns the raw block if it's owned.
    ///
    /// # Panics
    ///
    /// This method panics if the block is not owned and is in the cache.
    pub fn into_owned(self) -> Vec<u8> {
        match self {
            Self::Owned(b) => b,
            _ => panic!("cannot move block out of cache"),
        }
    }
}

impl Deref for BlockData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(b) => b.as_ref(),
            Self::Cached(b) => b.as_ref(),
        }
    }
}

impl From<Vec<u8>> for BlockData {
    fn from(b: Vec<u8>) -> Self {
        Self::Owned(b)
    }
}

impl From<CachedBlock> for BlockData {
    fn from(b: CachedBlock) -> Self {
        Self::Cached(b)
    }
}
