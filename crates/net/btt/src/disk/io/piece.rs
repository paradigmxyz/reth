use crate::{
    block::{block_count, block_len, CachedBlock},
    disk::{error::*, io::file::TorrentFile},
    info::FileIndex,
    sha1::Sha1Hash,
};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{self, Arc},
};
use tracing::{debug, warn};

/// An in-progress piece download that keeps in memory the so far downloaded
/// blocks and the expected hash of the piece.
pub(crate) struct Piece {
    /// The expected hash of the whole piece.
    pub expected_hash: Sha1Hash,
    /// The length of the piece, in bytes.
    pub len: u32,
    /// The so far downloaded blocks. Once the size of this map reaches the
    /// number of blocks in piece, the piece is complete and, if the hash is
    /// correct, saved to disk.
    ///
    /// Each block must be 16 KiB and is mapped to its offset within piece. A
    /// BTreeMap is used to keep blocks sorted by their offsets, which is
    /// important when iterating over the map to hash each block in the right
    /// order.
    // TODO: consider whether using a preallocated Vec of Options would be more
    // performant due to cache locality (we would have to count the missing
    // blocks though, or keep a separate counter)
    pub blocks: BTreeMap<u32, Vec<u8>>,
    /// The files that this piece overlaps with.
    ///
    /// This is a left-inclusive range of all all file indices, that can be used
    /// to index the `Torrent::files` vector to get the file handles.
    pub file_range: Range<FileIndex>,
}

impl Piece {
    /// Places block into piece's write buffer if it doesn't exist. TODO: should
    /// we return an error if it does?
    pub fn enqueue_block(&mut self, offset: u32, data: Vec<u8>) {
        use std::collections::btree_map::Entry;
        let entry = self.blocks.entry(offset);
        if matches!(entry, Entry::Occupied(_)) {
            warn!("Duplicate piece block at offset {}", offset);
        } else {
            entry.or_insert(data);
        }
    }

    /// Returns true if the piece has all its blocks in its write buffer.
    pub fn is_complete(&self) -> bool {
        self.blocks.len() == block_count(self.len)
    }

    /// Calculates the piece's hash using all its blocks and returns if it
    /// matches the expected hash.
    ///
    /// # Important
    ///
    /// This is potentially a computationally expensive function and should be
    /// executed on a thread pool and not the executor.
    pub fn matches_hash(&self) -> bool {
        // sanity check that we only call this method if we have all blocks in
        // piece
        debug_assert_eq!(self.blocks.len(), block_count(self.len));
        let mut hasher = Sha1::new();
        for block in self.blocks.values() {
            hasher.update(&block);
        }
        let hash = hasher.finalize();
        debug!("Piece hash: {:x}", hash);
        hash.as_slice() == self.expected_hash.as_ref()
    }

    /// Writes the piece's blocks to the files the piece overlaps with.
    ///
    /// # Important
    ///
    /// This performs sync IO and is thus potentially blocking and should be
    /// executed on a thread pool, and not the async executor.
    pub fn write(
        &self,
        torrent_piece_offset: u64,
        files: &[sync::RwLock<TorrentFile>],
    ) -> Result<(), WriteError> {
        todo!()
        // // convert the blocks to IO slices that the underlying
        // // systemcall can deal with
        // let mut blocks: Vec<_> =
        //     self.blocks.values().map(|b| IoVec::from_slice(b)).collect();
        // // the actual slice of blocks being worked on
        // let mut bufs = blocks.as_mut_slice();
        //
        // // loop through all files piece overlaps with and write that part of
        // // piece to file
        // let files = &files[self.file_range.clone()];
        // debug_assert!(!files.is_empty());
        // // the offset at which we need to write in torrent, which is updated
        // // with each write
        // let mut torrent_write_offset = torrent_piece_offset;
        // let mut total_write_count = 0;
        //
        // for file in files.iter() {
        //     let file = file.write().unwrap();
        //
        //     // determine which part of the file we need to write to
        //     debug_assert!(self.len as u64 > total_write_count);
        //     let remaining_piece_len = self.len as u64 - total_write_count;
        //     let file_slice = file
        //         .info
        //         .get_slice(torrent_write_offset, remaining_piece_len);
        //     // an empty file slice shouldn't occur as it would mean that piece
        //     // was thought to span fewer files than it actually does
        //     debug_assert!(file_slice.len > 0);
        //     // the write buffer should still contain bytes to write
        //     debug_assert!(!bufs.is_empty());
        //     debug_assert!(!bufs[0].as_slice().is_empty());
        //
        //     // write to file
        //     let tail = file.write(file_slice, bufs)?;
        //
        //     // `write_vectored_at` only writes at most `slice.len` bytes of
        //     // `bufs` to disk and returns the portion that wasn't
        //     // written, which we can use to set the write buffer for the next
        //     // round
        //     bufs = tail;
        //
        //     torrent_write_offset += file_slice.len as u64;
        //     total_write_count += file_slice.len;
        // }
        //
        // // we should have used up all write buffers (i.e. written all blocks to
        // // disk)
        // debug_assert!(bufs.is_empty());
    }
}

/// Reads a piece's blocks from the specified portion of the file from disk.
///
/// # Arguments
///
/// * `torrent_piece_offset` - The absolute offset of the piece's first byte in the whole torrent.
///   From this value the relative offset of piece within file is calculated.
/// * `file_range` - The files that contain data of the piece.
/// * `files` - A slice of all files in torrent.
/// * `len` - The length of the piece to read in.  While this function is currently used to read the
///   whole piece, it could also be used to read only a portion of the piece or several pieces with
///   this argument.
pub(super) fn read(
    torrent_piece_offset: u64,
    file_range: Range<FileIndex>,
    files: &[sync::RwLock<TorrentFile>],
    len: u32,
) -> Result<Vec<CachedBlock>, ReadError> {
    // reserve a read buffer for all blocks in piece
    let block_count = block_count(len);
    let mut blocks = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let block_len = block_len(len, i);
        let mut buf = Vec::new();
        buf.resize(block_len as usize, 0u8);
        blocks.push(Arc::new(buf))
    }

    // // convert the blocks to IO slices that the underlying
    // // systemcall can deal with
    // let mut iovecs: Vec<IoVec<&mut [u8]>> =
    //     blocks
    //         .iter_mut()
    //         .map(|b| {
    //             IoVec::from_mut_slice(Arc::get_mut(b).expect(
    //                 "cannot get mut ref to buffer only used by this thread",
    //             ).as_mut_slice())
    //         })
    //         .collect();
    // let mut bufs = iovecs.as_mut_slice();
    //
    // // loop through all files piece overlaps with and read that part of
    // // file
    // let files = &files[file_range];
    // debug_assert!(!files.is_empty());
    // let len = len as u64;
    // // the offset at which we need to read from torrent, which is updated
    // // with each read
    // let mut torrent_read_offset = torrent_piece_offset;
    // let mut total_read_count = 0;
    //
    // for file in files.iter() {
    //     let file = file.read().unwrap();
    //
    //     // determine which part of the file we need to read from
    //     debug_assert!(len > total_read_count);
    //     let remaining_piece_len = len - total_read_count;
    //     let file_slice = file
    //         .info
    //         .get_slice(torrent_read_offset, remaining_piece_len);
    //     // an empty file slice shouldn't occur as it would mean that piece
    //     // was thought to span fewer files than it actually does
    //     debug_assert!(file_slice.len > 0);
    //
    //     // read data
    //     bufs = file.read(file_slice, bufs)?;
    //
    //     torrent_read_offset += file_slice.len as u64;
    //     total_read_count += file_slice.len;
    // }
    //
    // // we should have read in the whole piece
    // debug_assert_eq!(total_read_count, len);

    // Ok(blocks)

    todo!()
}
