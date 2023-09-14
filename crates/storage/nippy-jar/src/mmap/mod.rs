#[allow(clippy::missing_safety_doc)]
pub use memmap2::*;

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{SealedBlock, H256};
    use reth_rlp::{Decodable, Encodable};
    use std::{fs::OpenOptions, io::Write};
    use tempfile::tempdir;

    /// General mmap JIT execution test
    #[test]
    fn exec_mmap_function() {
        let mov_x0_1: u32 = 0x200080D2; // MOV x0, #1
        let ret: u32 = 0xC0035FD6; // RET
        let mut mmap = MmapMut::map_anon(8).unwrap();
        mmap[4..8].copy_from_slice(&ret.to_be_bytes());

        // Build and execute a function that should return 1
        mmap[0..4].copy_from_slice(&mov_x0_1.to_be_bytes());
        let mmap_exec = mmap.make_exec().unwrap();
        let f: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(mmap_exec.as_ptr()) };
        assert_eq!(unsafe { f() }, 1);
    }

    /// Create & read mmap'ed file.
    #[test]
    fn read_mmaped_file() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let filename = "test";
        let contents = BytesMut::from(&[1; 100][..]);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(dir_path.join(filename))
            .unwrap();
        file.write_all(&contents).unwrap();

        let mmap = unsafe { Mmap::map(&file).unwrap() };
        assert_eq!(&contents[..], &mmap[..]);
    }

    /// Create snapshots with RLP encoded blocks and read them..
    #[test]
    fn read_mmaped_block_snapshots() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let blocks_per_snapshot: u64 = 100;
        let snapshots_count: u64 = 10;

        let mut rng = rand::thread_rng();

        let mut snapshots = Vec::with_capacity(snapshots_count as usize);
        for snapshot_idx in 0..snapshots_count {
            let start_block = snapshot_idx * blocks_per_snapshot;
            let end_block = start_block + blocks_per_snapshot - 1;

            let snapshot_filename = format!("snapshot-{start_block}-{end_block}");
            let filepath = dir_path.join(&snapshot_filename);
            let mut snapshot_file =
                OpenOptions::new().read(true).write(true).create(true).open(&filepath).unwrap();

            let blocks =
                random_block_range(&mut rng, start_block..=end_block, H256::default(), 0..6);

            let mut offset = 0;
            let mut offsets = Vec::with_capacity(blocks.len());
            for block in &blocks {
                let mut buf = Vec::new();
                block.encode(&mut buf);
                let len = snapshot_file.write(&buf).unwrap();
                offsets.push((offset, len));
                offset += len;
            }

            snapshots.push((snapshot_file, blocks, offsets));
            println!(
                "Wrote snapshot: {} {}b",
                filepath.display(),
                filepath.metadata().unwrap().len()
            )
        }

        for (snapshot, blocks, offsets) in snapshots {
            let mmap = unsafe { Mmap::map(&snapshot).unwrap() };

            for (block, (offset, len)) in blocks.iter().zip(offsets) {
                let mut block_slice = &mmap[offset..offset + len];
                assert_eq!(Ok(block), SealedBlock::decode(&mut block_slice).as_ref());
            }
        }
    }
}
