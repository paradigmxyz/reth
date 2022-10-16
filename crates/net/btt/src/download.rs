use crate::{
    block::{block_count, block_len, BlockInfo, BLOCK_LEN},
    info::PieceIndex,
};
use std::collections::HashSet;
use tracing::trace;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BlockStatus {
    Free,
    Requested,
    Received,
}

impl Default for BlockStatus {
    fn default() -> Self {
        Self::Free
    }
}

/// Tracks the completion of an ongoing piece download and is used to request
/// missing blocks in piece.
pub(crate) struct PieceDownload {
    /// The piece's index.
    index: PieceIndex,
    /// The piece's length in bytes.
    len: u32,
    /// The blocks in this piece, tracking which are downloaded, pending, or
    /// received. The vec is preallocated to the number of blocks in piece.
    blocks: Vec<BlockStatus>,
}

impl PieceDownload {
    /// Creates a new piece download instance for the given piece.
    pub(crate) fn new(index: PieceIndex, len: u32) -> Self {
        let block_count = block_count(len);
        let mut blocks = Vec::new();
        blocks.resize_with(block_count, Default::default);
        Self { index, len, blocks }
    }

    /// Returns the index of the piece that is downloaded.
    pub(crate) fn piece_index(&self) -> PieceIndex {
        self.index
    }

    /// Picks the requested number of blocks or fewer, if fewer are remaining.
    /// If we're in end game mode, we ignore blocks requested by other peers.
    pub(crate) fn pick_blocks(
        &mut self,
        count: usize,
        pick_buf: &mut Vec<BlockInfo>,
        in_end_game: bool,
        prev_picked: &HashSet<BlockInfo>,
    ) {
        trace!(
            "Trying to pick {} block(s) in piece {} (length: {}, blocks: {})",
            count,
            self.index,
            self.len,
            self.blocks.len(),
        );

        let mut picked = 0;

        for (i, block) in self.blocks.iter_mut().enumerate() {
            // don't pick more than requested
            if picked == count {
                break
            }

            // only pick block if it's free
            if *block == BlockStatus::Free {
                pick_buf.push(BlockInfo {
                    piece_index: self.index,
                    offset: i as u32 * BLOCK_LEN,
                    len: block_len(self.len, i),
                });
                *block = BlockStatus::Requested;
                picked += 1;
            } else if in_end_game && *block == BlockStatus::Requested {
                // in endgame it's fair to pick blocks already requested but
                // don't pick the same block twice from the same peer
                let block_info = BlockInfo {
                    piece_index: self.index,
                    offset: i as u32 * BLOCK_LEN,
                    len: block_len(self.len, i),
                };
                // TODO: we could probably optimize this by saving some cheap
                // peer id in the block metadata and check if peer is present
                // (we'll need something like this for parole downloads at some
                // point anyway)
                if !prev_picked.contains(&block_info) {
                    pick_buf.push(block_info);
                    picked += 1;
                }
            }

            // TODO(https://github.com/mandreyel/cratetorrent/issues/18): if we
            // requested block too long ago, time out block
        }

        if picked > 0 {
            trace!(
                "Picked {} block(s) for piece {}: {:?}",
                picked,
                self.index,
                &pick_buf[pick_buf.len() - picked..]
            );
        } else {
            trace!("Cannot pick any blocks in piece {}", self.index);
        }
    }

    /// Marks the given block as received so that it is not picked again.
    ///
    /// The previous status of the block is returned. This can be used to check
    /// whether the block has already been downloaded, for example.
    pub(crate) fn received_block(&mut self, block: &BlockInfo) -> BlockStatus {
        trace!("Received piece {} block {:?}", self.index, block);

        // TODO(https://github.com/mandreyel/cratetorrent/issues/16): this
        // information is sanitized in PeerSession but maybe we want to return
        // a Result anyway
        debug_assert_eq!(block.piece_index, self.index);
        debug_assert!(block.offset < self.len);
        debug_assert!(block.len <= self.len);

        // we should only receive blocks that we have requested before
        debug_assert!(matches!(self.blocks[block.index_in_piece()], BlockStatus::Requested));

        // TODO(https://github.com/mandreyel/cratetorrent/issues/9): record
        // rount trip time for this block

        let block = &mut self.blocks[block.index_in_piece()];
        let prev_status = *block;
        *block = BlockStatus::Received;
        prev_status
    }

    /// Marks all blocks free to be requested again.
    pub(crate) fn free_all_blocks(&mut self) {
        trace!("Canceling all blocks in piece {}", self.index);
        for block in self.blocks.iter_mut() {
            *block = BlockStatus::Free;
        }
    }

    /// Marks a previously requested block free to request again.
    pub(crate) fn free_block(&mut self, block: &BlockInfo) {
        trace!("Canceling request for piece {} block {:?}", self.index, block);

        // TODO(https://github.com/mandreyel/cratetorrent/issues/16): this
        // information is sanitized in PeerSession but maybe we want to return
        // a Result anyway
        debug_assert_eq!(block.piece_index, self.index);
        debug_assert!(block.offset < self.len);
        debug_assert!(block.len <= self.len);

        self.blocks[block.index_in_piece()] = BlockStatus::Free;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Tests that repeatedly requesting as many blocks as are in the piece
    /// returns all blocks, none of them previously picked.
    #[test]
    fn should_pick_all_blocks_one_by_one() {
        let index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let block_count = block_count(piece_len);
        let in_end_game = false;

        let mut download = PieceDownload::new(index, piece_len);
        // save picked blocks
        let mut picked = HashSet::with_capacity(block_count);

        // pick all blocks one by one
        for _ in 0..block_count {
            let mut picked_blocks = Vec::new();
            download.pick_blocks(1, &mut picked_blocks, in_end_game, &picked);
            assert_eq!(picked_blocks.len(), 1);
            let block = *picked_blocks.first().unwrap();
            // assert that this block hasn't been picked before
            assert!(!picked.contains(&block));
            // mark block as picked
            picked.insert(block);
        }

        // assert that we picked all blocks
        assert_eq!(picked.len(), block_count);
        for block in download.blocks.iter() {
            assert!(matches!(block, BlockStatus::Requested));
        }
    }

    /// Tests that requesting as many blocks as are in the piece in one go
    /// returns all blocks.
    #[test]
    fn should_pick_all_blocks_in_new_download() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let in_end_game = false;

        let mut download = PieceDownload::new(piece_index, piece_len);

        // pick all blocks
        let block_count = block_count(piece_len);
        let mut picked_blocks = Vec::new();
        download.pick_blocks(block_count, &mut picked_blocks, in_end_game, &HashSet::new());
        assert_eq!(picked_blocks.len(), block_count);

        // assert that we picked all blocks
        for block in download.blocks.iter() {
            assert!(matches!(block, BlockStatus::Requested));
        }
    }

    /// Tests that after marking a block as received we don't pick those blocks
    /// again.
    #[test]
    fn should_not_pick_received_blocks() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let block_count = block_count(piece_len);
        let in_end_game = false;

        let mut download = PieceDownload::new(piece_index, piece_len);

        let mut picked_blocks = Vec::new();
        download.pick_blocks(block_count, &mut picked_blocks, in_end_game, &HashSet::new());
        assert_eq!(picked_blocks.len(), block_count);

        // mark all blocks as requested
        for block in picked_blocks.iter() {
            download.received_block(block);
        }

        let mut picked_blocks = Vec::new();
        download.pick_blocks(block_count, &mut picked_blocks, in_end_game, &HashSet::new());
        assert!(picked_blocks.is_empty());
    }

    /// Tests that requesting as many blocks as are in the piece in one go
    /// returns only blocks not already requested or received.
    #[test]
    fn should_pick_only_free_blocks_from_all() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let in_end_game = false;

        let mut download = PieceDownload::new(piece_index, piece_len);

        // pick 4 blocks
        let picked_block_indices = [0, 1, 2, 3];
        let mut picked_blocks = Vec::new();
        download.pick_blocks(
            picked_block_indices.len(),
            &mut picked_blocks,
            in_end_game,
            &HashSet::new(),
        );
        assert_eq!(picked_blocks.len(), picked_block_indices.len());

        // mark 3 of them as received
        let received_block_count = 3;
        for block in picked_blocks.iter().take(received_block_count) {
            download.received_block(block);
        }

        let block_count = block_count(piece_len);

        assert_eq!(
            download.blocks.iter().fold(0, |acc, block| {
                if !matches!(block, BlockStatus::Received) {
                    acc + 1
                } else {
                    acc
                }
            }),
            block_count - received_block_count
        );

        // pick all remaining free blocks
        let mut picked_blocks = Vec::new();
        download.pick_blocks(block_count, &mut picked_blocks, in_end_game, &HashSet::new());
        assert_eq!(picked_blocks.len(), block_count - picked_block_indices.len());
    }

    /// Tests that in endgame mode blocks that were already picked by other
    /// peers can be picked by other peers again.
    #[test]
    fn should_pick_requested_blocks_again_in_end_game() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let block_count = block_count(piece_len);
        let in_end_game = true;

        let mut download = PieceDownload::new(piece_index, piece_len);

        // pick all blocks multiple times
        for _ in 0..2 {
            let mut picked_blocks = Vec::new();
            download.pick_blocks(block_count, &mut picked_blocks, in_end_game, &HashSet::new());
            assert_eq!(picked_blocks.len(), block_count);
        }
    }

    /// Tests that blocks that were already picked by a peer are not picked
    /// again for the same peer (only relevant in endgame mode).
    #[test]
    fn should_not_pick_already_picked_blocks_in_end_game() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;
        let block_count = block_count(piece_len);
        let in_end_game = true;

        let mut download = PieceDownload::new(piece_index, piece_len);
        // save picked blocks
        let mut picked = HashSet::with_capacity(block_count);

        // pick all blocks one by one
        for _ in 0..block_count {
            let mut picked_blocks = Vec::new();
            download.pick_blocks(1, &mut picked_blocks, in_end_game, &picked);
            assert_eq!(picked_blocks.len(), 1);
            let block = *picked_blocks.first().unwrap();
            // assert that this block hasn't been picked before
            assert!(!picked.contains(&block));
            // mark block as picked
            picked.insert(block);
        }
    }
}
