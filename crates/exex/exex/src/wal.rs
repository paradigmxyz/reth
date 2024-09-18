#![allow(dead_code)]

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use reth_exex_types::ExExNotification;
use reth_primitives::BlockNumHash;
use reth_provider::Chain;
use reth_tracing::tracing::debug;

/// The maximum number of blocks to cache in the WAL.
///
/// [`CachedBlock`] has a size of `u64 + u64 + B256` which is 384 bits. 384 bits * 1 million = 48
/// megabytes.
const MAX_CACHED_BLOCKS: usize = 1_000_000;

#[derive(Debug)]
struct CachedBlock {
    /// The file offset where the WAL entry is written.
    file_offset: u64,
    /// The block number and hash of the block.
    block: BlockNumHash,
}

#[derive(Debug)]
pub(crate) struct Wal {
    /// The path to the WAL file.
    path: PathBuf,
    /// The file handle of the WAL file.
    file: File,
    /// The block cache of the WAL. Acts as a FIFO queue with a maximum size of
    /// [`MAX_CACHED_BLOCKS`].
    ///
    /// For each notification written to the WAL, there will be an entry per block written to
    /// the cache with the same file offset as the notification in the WAL file.
    block_cache: VecDeque<CachedBlock>,
}

impl Wal {
    /// Creates a new instance of [`Wal`].
    pub(crate) fn new(directory: PathBuf) -> eyre::Result<Self> {
        let path = directory.join("latest.wal");
        let file = File::create(&path)?;

        let mut wal = Self { path, file, block_cache: VecDeque::new() };
        wal.fill_block_cache(u64::MAX)?;

        Ok(wal)
    }

    /// Fills the block cache with the notifications from the WAL file, up to the given offset in
    /// bytes, not inclusive.
    fn fill_block_cache(&mut self, to_offset: u64) -> eyre::Result<()> {
        self.block_cache = VecDeque::new();

        let mut file_offset = 0;
        for line in BufReader::new(&self.file).split(b'\n') {
            let line = line?;
            let chain: Chain = bincode::deserialize(&line)?;
            for block in chain.blocks().values() {
                self.block_cache.push_back(CachedBlock {
                    file_offset,
                    block: (block.number, block.hash()).into(),
                });
                if self.block_cache.len() > MAX_CACHED_BLOCKS {
                    self.block_cache.pop_front();
                }
            }

            file_offset += line.len() as u64 + 1;
            if file_offset >= to_offset {
                break
            }
        }

        Ok(())
    }

    /// Commits the notification to WAL. If the notification contains a
    /// reverted chain, the WAL is truncated.
    pub(crate) fn commit(&mut self, notification: &ExExNotification) -> eyre::Result<()> {
        if let Some(reverted_chain) = notification.reverted_chain() {
            let mut truncate_to = None;
            let mut reverted_blocks = reverted_chain.blocks().values().rev();
            loop {
                let Some(block) = self.block_cache.pop_back() else {
                    self.fill_block_cache(truncate_to.unwrap_or(u64::MAX))?;
                    if self.block_cache.is_empty() {
                        break
                    }
                    continue
                };

                let Some(reverted_block) = reverted_blocks.next() else { break };

                if reverted_block.number != block.block.number ||
                    reverted_block.hash() != block.block.hash
                {
                    return Err(eyre::eyre!("inconsistent WAL block cache entry"))
                }

                truncate_to = Some(block.file_offset);
            }

            if let Some(truncate_to) = truncate_to {
                self.file.set_len(truncate_to)?;
            }
        }

        if let Some(committed_chain) = notification.committed_chain() {
            let data = bincode::serialize(&notification)?;

            let file_offset = self.file.metadata()?.len();
            self.file.write_all(&data)?;
            self.file.write_all(b"\n")?;
            self.file.flush()?;

            for block in committed_chain.blocks().values() {
                self.block_cache.push_back(CachedBlock {
                    file_offset,
                    block: (block.number, block.hash()).into(),
                });
                if self.block_cache.len() > MAX_CACHED_BLOCKS {
                    self.block_cache.pop_front();
                }
            }
        }

        Ok(())
    }

    /// Rollbacks the WAL to the given block, inclusive. Returns the block number and hash of the
    /// lowest removed block.
    pub(crate) fn rollback(
        &mut self,
        to_block: BlockNumHash,
    ) -> eyre::Result<Option<BlockNumHash>> {
        let mut truncate_to = None;
        let mut lowest_removed_block = None;
        loop {
            let Some(block) = self.block_cache.pop_back() else {
                self.fill_block_cache(truncate_to.unwrap_or(u64::MAX))?;
                if self.block_cache.is_empty() {
                    break
                }
                continue
            };

            if block.block.number == to_block.number {
                if block.block.hash != to_block.hash {
                    return Err(eyre::eyre!("block hash mismatch in WAL"))
                }

                truncate_to = Some(block.file_offset);

                self.file.seek(SeekFrom::Start(block.file_offset))?;
                let notification: ExExNotification = bincode::deserialize_from(&self.file)?;
                lowest_removed_block = notification
                    .committed_chain()
                    .as_ref()
                    .map(|chain| chain.first())
                    .map(|block| (block.number, block.hash()).into());

                break
            }

            truncate_to = Some(block.file_offset);
        }

        if let Some(truncate_to) = truncate_to {
            self.file.set_len(truncate_to)?;
        }

        Ok(lowest_removed_block)
    }

    /// Finalizes the WAL to the given block, inclusive.
    pub(crate) fn finalize(&mut self, to_block: BlockNumHash) -> eyre::Result<()> {
        // Find an offset where the unfinalized blocks start. If the notificatin includes both
        // finalized and non-finalized blocks, it will not be truncated and the offset will
        // include it.

        // First, walk cache to find the offset of the notification with the finalized block.
        let mut unfinalized_from_offset = None;
        while let Some(cached_block) = self.block_cache.pop_front() {
            if cached_block.block.number == to_block.number {
                if cached_block.block.hash != to_block.hash {
                    return Err(eyre::eyre!("block hash mismatch in WAL"))
                }

                unfinalized_from_offset = Some(self.block_cache.front().map_or_else(
                    || std::io::Result::Ok(self.file.metadata()?.len()),
                    |block| std::io::Result::Ok(block.file_offset),
                )?);
                break
            }
        }

        // If the finalized block is not found in cache, we need to walk the whole file.
        if unfinalized_from_offset.is_none() {
            let mut offset = 0;
            for data in BufReader::new(&self.file).split(b'\n') {
                let data = data?;
                let notification: ExExNotification = bincode::deserialize(&data)?;
                if let Some(committed_chain) = notification.committed_chain() {
                    let finalized_block = committed_chain.blocks().get(&to_block.number);

                    if let Some(finalized_block) = finalized_block {
                        if finalized_block.hash() != to_block.hash {
                            return Err(eyre::eyre!("block hash mismatch in WAL"))
                        }

                        if committed_chain.blocks().len() == 1 {
                            unfinalized_from_offset = Some(offset + data.len() as u64 + 1);
                        } else {
                            unfinalized_from_offset = Some(offset);
                        }

                        break
                    }
                }

                offset += data.len() as u64 + 1;
            }
        }

        // If the finalized block is still not found, we can't do anything and just return.
        let Some(unfinalized_from_offset) = unfinalized_from_offset else {
            debug!(target: "exex::wal", ?to_block, "Could not find the finalized block in WAL");
            return Ok(())
        };

        // Rename the existing WAL file to a temporary one.
        let tmp_file_path = self.path.with_extension("tmp");
        reth_fs_util::rename(&self.path, &tmp_file_path)?;

        // Open the temporary file for reading and seek to the first notification containing an
        // unfinalized block.
        let mut old_file = File::open(&tmp_file_path)?;
        old_file.seek(SeekFrom::Start(unfinalized_from_offset))?;

        // Copy the notifications with unfinalized blocks to the new file.
        let mut new_file = File::create(&self.path)?;
        loop {
            let mut buffer = [0; 4096];
            let read = old_file.read(&mut buffer)?;
            new_file.write_all(&buffer[..read])?;

            if read < 1024 {
                break
            }
        }

        // Remove the temporary file and update the file handle with the new one.
        reth_fs_util::remove_file(tmp_file_path)?;
        self.file = new_file;

        // Fill the block cache with the notifications from the new file.
        self.fill_block_cache(u64::MAX)?;

        Ok(())
    }
}
