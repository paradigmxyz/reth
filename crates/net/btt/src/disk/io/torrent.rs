use crate::{
    block::{Block, BlockInfo, CachedBlock},
    disk::{
        error::*,
        io::{
            file::TorrentFile,
            piece::{self, Piece},
        },
    },
    info::{PieceIndex, StorageInfo},
    peer,
    torrent::{self, PieceCompletion},
};
use lru::LruCache;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    num::NonZeroUsize,
    sync::{
        self,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::task;
use tracing::{debug, error, info, trace, warn};

/// Torrent information related to disk IO.
///
/// Contains the in-progress pieces (i.e. the write buffer), metadata about
/// torrent's download and piece sizes, etc.
pub(crate) struct Torrent {
    /// All information concerning this torrent's storage.
    info: StorageInfo,

    /// The in-progress piece downloads and disk writes. This is the torrent's
    /// disk write buffer. Each piece is mapped to its index for faster lookups.
    // TODO(https://github.com/mandreyel/cratetorrent/issues/22): Currently
    // there is no upper bound on this.
    write_buf: HashMap<PieceIndex, Piece>,

    /// Contains the fields that may be accessed by other threads.
    ///
    /// This is an optimization to avoid having to call
    /// `Arc::clone(&self.field)` for each of the contained fields when sending
    /// them to an IO worker threads. See more in [`ThreadContext`].
    thread_ctx: Arc<ThreadContext>,

    /// The concatenation of all expected piece hashes.
    piece_hashes: Vec<u8>,
}

/// Contains fields that are commonly accessed by torrent's IO threads.
///
/// We're using blocking IO to read things from disk and so such operations need to be
/// spawned on a separate thread to not block the tokio reactor driving the
/// disk task.
/// But these threads need some fields from torrent and so those fields
/// would need to be in an arc each. With this optimization, only this
/// struct needs to be in an arc and thus only a single atomic increment has to
/// be made when sending the contained fields across threads.
struct ThreadContext {
    /// The channel used to alert a torrent that a block has been written to
    /// disk and/or a piece was completed.
    tx: torrent::Sender,

    /// The read cache that caches entire pieces.
    ///
    /// The piece is stored as a list of 16 KiB blocks since that is what peers
    /// are going to request, so this avoids extra copies. Blocks are ordered.
    ///
    /// Every time a block read is issued it is checked if it's already cached
    /// here. If not, the whole pieces is read from disk and placed in the cache.
    ///
    /// # Sync mutex
    ///
    /// The cache is behind a synchronous mutex, instead of using tokio's async
    /// locks. This is because it is being accessed by an async and a blocking
    /// task (where an async mutex cannot be used). However, it is safe to do so
    /// as the lock guards are never held across suspension points. In fact, the
    /// tokio redis example recommends using sync mutexes if holding them across
    /// await points is not needed:
    /// https://github.com/tokio-rs/mini-redis/blob/master/src/db.rs#L29.
    /// https://docs.rs/tokio/0.2.24/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use
    ///
    /// Locking occurs in two places:
    /// - once when reading the cache on the async task,
    /// - and once when inserting a new entry in the blocking task.
    ///
    /// Both of these are very short lived and shouldn't bog down the reactor by
    /// too much.
    ///
    /// TODO: An improvement might be to use a concurrent LRU cache or to
    /// not update the cache state on reads but to periodically do so via a
    /// timer or similar.
    read_cache: sync::Mutex<LruCache<PieceIndex, Vec<CachedBlock>>>,

    /// Handles of all files in torrent, opened in advance during torrent
    /// creation.
    ///
    /// Each writer thread will get exclusive access to the file handle it
    /// needs, referring to it directly in the vector (hence the arc).
    /// Multiple readers may read from the same file, but not while there is
    /// a pending write.
    ///
    /// Later we will need to make file access more granular, as multiple
    /// concurrent writes to the same file that don't overlap are safe to do.
    // TODO: consider improving concurreny by allowing concurrent reads and
    // writes on different parts of the file using byte-range locking
    // TODO: Is there a way to avoid copying `FileInfo`s here from
    // `self.info.structure`? We could just pass the file info on demand, but
    // that woudl require reallocating this vector every time (to pass a new
    // vector of pairs of `TorrentFile` and `FileInfo`).
    files: Vec<sync::RwLock<TorrentFile>>,

    /// Various disk IO related statistics.
    ///
    /// Stats are atomically updated by the IO worker threads themselves.
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    /// The number of bytes successfully written to disk.
    write_count: AtomicU64,
    /// The number of times we failed to write to disk.
    write_failure_count: AtomicUsize,
    /// The number of bytes successfully read from disk.
    read_count: AtomicU64,
    /// The number of times we failed to read from disk.
    read_failure_count: AtomicUsize,
}

impl Torrent {
    /// handles.
    /// Creates the file system structure of the torrent and opens the file
    ///
    /// For a single file, there is a path validity check and then the file is
    /// opened. For multi-file torrents, if there are any subdirectories in the
    /// torrent archive, they are created and all files are opened.
    pub fn new(
        info: StorageInfo,
        piece_hashes: Vec<u8>,
        torrent_tx: torrent::Sender,
    ) -> Result<Self, NewTorrentError> {
        // TODO: since this is done as part of a tokio::task, should we use
        // tokio_fs here?
        if !info.download_dir.is_dir() {
            warn!("Creating missing download directory {:?}", info.download_dir);
            fs::create_dir_all(&info.download_dir)?;
            info!("Download directory {:?} created", info.download_dir);
        }

        // TODO: return error instead
        debug_assert_ne!(info.files.len(), 0, "torrent must have files");
        let files = if info.files.len() == 1 {
            let file = &info.files[0];
            debug!("Torrent is single {} bytes long file {:?}", file.len, file.path);
            vec![sync::RwLock::new(TorrentFile::new(&info.download_dir, file.clone())?)]
        } else {
            debug_assert!(!info.files.is_empty());
            debug!("Torrent is multi file: {:?}", info.files);
            debug!("Setting up directory structure");

            let mut torrent_files = Vec::with_capacity(info.files.len());
            for file in info.files.iter() {
                let path = info.download_dir.join(&file.path);
                // get the parent of the file path: if there is one (i.e.
                // this is not a file in the torrent root), and doesn't
                // exist, create it
                if let Some(subdir) = path.parent() {
                    if !subdir.exists() {
                        info!("Creating torrent subdir {:?}", subdir);
                        fs::create_dir_all(&subdir).map_err(|e| {
                            error!("Failed to create subdir {:?}", subdir);
                            NewTorrentError::Io(e)
                        })?;
                    }
                }

                // open the file and get a handle to it
                torrent_files
                    .push(sync::RwLock::new(TorrentFile::new(&info.download_dir, file.clone())?));
            }
            torrent_files
        };

        Ok(Self {
            info,
            write_buf: HashMap::new(),
            thread_ctx: Arc::new(ThreadContext {
                tx: torrent_tx,
                read_cache: sync::Mutex::new(LruCache::new(
                    NonZeroUsize::new(READ_CACHE_UPPER_BOUND).unwrap(),
                )),
                files,
                stats: Stats::default(),
            }),
            piece_hashes,
        })
    }

    pub fn write_block(&mut self, info: BlockInfo, data: Vec<u8>) -> Result<()> {
        trace!("Saving block {} to disk", info);

        let piece_index = info.piece_index;
        if !self.write_buf.contains_key(&piece_index) {
            self.start_new_piece(info.piece_index);
        }
        let piece = self.write_buf.get_mut(&piece_index).expect("Newly inserted piece not present");

        piece.enqueue_block(info.offset, data);

        // if the piece has all its blocks, it means we can hash it and save it
        // to disk and clear its write buffer
        if piece.is_complete() {
            // TODO: remove from in memory store only if the disk write
            // succeeded (otherwise we need to retry later)
            let piece = self.write_buf.remove(&piece_index).unwrap();

            debug!(
                "Piece {} is complete ({} bytes), flushing {} block(s) to disk",
                info.piece_index,
                piece.len,
                piece.blocks.len()
            );

            // don't block the reactor with the potentially expensive hashing
            // and sync file writing
            let torrent_piece_offset = self.info.torrent_piece_offset(piece_index);
            let ctx = Arc::clone(&self.thread_ctx);
            task::spawn_blocking(move || {
                let is_piece_valid = piece.matches_hash();

                // save piece to disk if it's valid
                if is_piece_valid {
                    debug!("Piece {} is valid, writing to disk", piece_index);

                    if let Err(e) = piece.write(torrent_piece_offset, &*ctx.files) {
                        error!("Error writing piece {} to disk: {}", piece_index, e);
                        // TODO(https://github.com/mandreyel/cratetorrent/issues/23):
                        // also place back piece write buffer in torrent and
                        // retry later
                        ctx.stats.write_failure_count.fetch_add(1, Ordering::Relaxed);
                        // alert torrent of block write failure
                        ctx.tx
                            .send(torrent::TorrentCommand::PieceCompletion(Err(e)))
                            .map_err(|e| {
                                error!("Error sending piece result: {}", e);
                                e
                            })
                            .ok();
                        return
                    }

                    debug!("Wrote piece {} to disk", piece_index);
                    ctx.stats.write_count.fetch_add(piece.len as u64, Ordering::Relaxed);
                } else {
                    warn!("Piece {} is not valid", info.piece_index);
                }

                // alert torrent of piece completion and hash result
                ctx.tx
                    .send(torrent::TorrentCommand::PieceCompletion(Ok(PieceCompletion {
                        index: piece_index,
                        is_valid: is_piece_valid,
                    })))
                    .map_err(|e| {
                        error!("Error sending piece result: {}", e);
                        e
                    })
                    .ok();
            });
        }

        Ok(())
    }

    /// Starts a new in-progress piece, creating metadata for it in self.
    ///
    /// This involves getting the expected hash of the piece, its length, and
    /// calculating the files that it intersects.
    fn start_new_piece(&mut self, piece_index: PieceIndex) {
        trace!("Creating piece {} write buffer", piece_index);

        assert!(piece_index < self.info.piece_count, "piece index is invalid");

        // get the position of the piece in the concatenated hash string
        let hash_pos = piece_index * 20;
        // the above assert should take care of this, but just in case
        debug_assert!(hash_pos + 20 <= self.piece_hashes.len());

        let hash_slice = &self.piece_hashes[hash_pos..hash_pos + 20];
        let mut expected_hash = [0; 20];
        expected_hash.copy_from_slice(hash_slice);
        debug!("Piece {} expected hash {}", piece_index, hex::encode(&expected_hash));

        let len = self.info.piece_len(piece_index);
        debug!("Piece {} is {} bytes long", piece_index, len);

        let file_range = self.info.files_intersecting_piece(piece_index);
        debug!("Piece {} intersects files: {:?}", piece_index, file_range);

        let piece =
            Piece { expected_hash: expected_hash.into(), len, blocks: BTreeMap::new(), file_range };
        self.write_buf.insert(piece_index, piece);
    }

    /// Returns the specified block via the sender, either from the read cache
    /// or from the disk.
    ///
    /// If the block info refers to an invalid piece, an error is returned.
    /// If the block info is correct but the underlying file does not yet
    /// contain the data, an error is returned.
    ///
    /// On a cache miss, the method reads in the whole piece of the block,
    /// stores the piece in memory, and returns the requested block via the
    /// sender. The rationale is that if a peer is requesting a block in piece,
    /// it will very likely request further blocks in the same piece, so we want
    /// to prepare for it. This is referred to as a "read cache line", much like
    /// how the CPU pulls in the next 64 bytes of the program into its L1 cache
    /// when hitting a cache miss.
    /// For now, this is simplified in that we don't pull in blocks from the
    /// next piece. Later, we will make the read cache line size configurable
    /// and it will be applied across piece boundaries.
    pub fn read_block(&self, block_info: BlockInfo, result_tx: peer::Sender) -> Result<()> {
        trace!("Reading {} from disk", block_info);

        let piece_index = block_info.piece_index;
        let block_index = block_info.index_in_piece();

        // check if piece is in the read cache
        if let Some(blocks) = self.thread_ctx.read_cache.lock().unwrap().get(&piece_index) {
            debug!("Piece {} is in the read cache", piece_index);
            // the block's index in piece may be invalid
            if block_index >= blocks.len() {
                debug!("Piece {} block offset {} is invalid", piece_index, block_info.offset);
                self.thread_ctx.tx.send(torrent::TorrentCommand::ReadError {
                    block_info,
                    error: ReadError::InvalidBlockOffset,
                })?;
                // the disk task itself mustn't be aborted due to invalid input
                return Ok(())
            }

            // return block via sender
            let block = Arc::clone(&blocks[block_index]);
            result_tx.send(peer::PeerCommand::Block(Block::new(block_info, block)))?;
        } else {
            // otherwise read in the piece from disk
            debug!("Piece {} not in the read cache, reading from disk", piece_index);

            let file_range = self.info.files_intersecting_piece(piece_index);

            // Checking if the file pointed to by info has been downloaded yet
            // is done implicitly as part of the read operation below: if we
            // can't read any bytes, the file likely does not exist.

            // don't block the reactor with blocking disk IO
            let torrent_piece_offset = self.info.torrent_piece_offset(piece_index);
            let piece_len = self.info.piece_len(piece_index);
            let ctx = Arc::clone(&self.thread_ctx);
            task::spawn_blocking(move || {
                match piece::read(torrent_piece_offset, file_range, &ctx.files[..], piece_len) {
                    Ok(blocks) => {
                        debug!("Read piece {}", piece_index);
                        // pick requested block
                        let block = Arc::clone(&blocks[block_index]);

                        // Place piece in read cache. Another concurrent read
                        // could already have read the piece just before this
                        // thread, but replacing it shouldn't be an issue since
                        // we're reading the same data.
                        ctx.read_cache.lock().unwrap().put(piece_index, blocks);
                        ctx.stats.read_count.fetch_add(piece_len as u64, Ordering::Relaxed);

                        // send block to peer
                        result_tx
                            .send(peer::PeerCommand::Block(Block::new(block_info, block)))
                            .map_err(|e| {
                                error!("Error sending block to peer: {}", e);
                                e
                            })
                            .ok();
                    }
                    Err(e) => {
                        error!("Error reading piece {} from disk: {}", piece_index, e);

                        ctx.stats.read_failure_count.fetch_add(1, Ordering::Relaxed);
                        ctx.tx
                            .send(torrent::TorrentCommand::ReadError { block_info, error: e })
                            .map_err(|e| {
                                error!("Error sending read error: {}", e);
                                e
                            })
                            .ok();
                    }
                }
            });
        }

        Ok(())
    }
}

// TODO(https://github.com/mandreyel/cratetorrent/issues/22):
// make this configurable
const READ_CACHE_UPPER_BOUND: usize = 1000;
