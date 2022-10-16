pub(crate) mod file;
pub(crate) mod piece;
pub(crate) mod torrent;

#[cfg(test)]
mod tests {
    use crate::{
        block::BLOCK_LEN,
        disk::{
            error::*,
            io::{
                file::TorrentFile,
                piece::{self, Piece},
            },
        },
        info::{FileIndex, FileInfo},
    };
    use sha1::{Digest, Sha1};
    use std::{
        collections::BTreeMap,
        fs,
        io::Read,
        ops::Range,
        path::{Path, PathBuf},
        sync,
    };

    const DOWNLOAD_DIR: &str = "/tmp";

    // /// Tests that writing blocks to a single file using `TorrentFile` works.
    // #[test]
    // fn should_write_blocks_to_torrent_file() {
    //     let file_range = 0..1;
    //     let piece = make_piece(file_range);
    //
    //     let download_dir = Path::new(DOWNLOAD_DIR);
    //     let mut file = TorrentFile::new(
    //         download_dir,
    //         FileInfo {
    //             path: PathBuf::from("TorrentFile_write_block.test"),
    //             torrent_offset: 0,
    //             len: 2 * piece.len as u64,
    //         },
    //     )
    //     .expect("cannot create test file");
    //
    //     // write buffers
    //     let file_slice = file.info.get_slice(0, piece.len as u64);
    //     let mut iovecs: Vec<_> = piece
    //         .blocks
    //         .values()
    //         .map(|b| IoVec::from_slice(b))
    //         .collect();
    //     let tail = file
    //         .write(file_slice, &mut iovecs)
    //         .expect("cannot write piece to file");
    //     assert!(tail.is_empty(), "not all blocks were written to disk");
    //
    //     // read and compare
    //     let mut file_content = Vec::new();
    //     file.handle
    //         .read_to_end(&mut file_content)
    //         .expect("cannot read test file");
    //     assert_eq!(
    //         file_content,
    //         piece.blocks.values().cloned().flatten().collect::<Vec<_>>(),
    //         "file content does not equal piece"
    //     );
    //
    //     // clean up env
    //     fs::remove_file(download_dir.join(&file.info.path))
    //         .expect("cannot remove test file");
    // }

    /// Tests that writing piece to a single file works.
    #[test]
    fn should_write_piece_to_single_file() {
        let file_range = 0..1;
        let piece = make_piece(file_range);
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_single_file.test"),
                torrent_offset: 0,
                len: 2 * piece.len as u64,
            },
        )
        .expect("cannot create test file");
        let files = &[sync::RwLock::new(file)];

        // piece starts at the beginning of files
        let torrent_piece_offset = 0;
        piece.write(torrent_piece_offset, files).expect("cannot write piece to file");

        // compare file content to piece
        let mut file = files[0].write().unwrap();
        let mut file_content = Vec::new();
        file.handle.read_to_end(&mut file_content).expect("cannot read test file");
        assert_eq!(
            file_content,
            piece.blocks.values().flatten().cloned().collect::<Vec<_>>(),
            "file {:?} content does not equal piece",
            file.info
        );

        // clean up env
        fs::remove_file(download_dir.join(&file.info.path)).expect("cannot remove test file");
    }

    #[test]
    fn should_not_read_piece_from_empty_file() {
        let file_range = 0..1;
        let piece = make_piece(file_range.clone());
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_read_empty_single_file_error.test"),
                torrent_offset: 0,
                len: 2 * piece.len as u64,
            },
        )
        .expect("cannot create test file");
        let files = &[sync::RwLock::new(file)];

        // reading piece from empty file should result in error
        let torrent_piece_offset = 0;
        let result = piece::read(torrent_piece_offset, file_range, files, piece.len);
        assert!(matches!(result, Err(ReadError::MissingData)));

        // clean up env
        fs::remove_file(download_dir.join(&files[0].read().unwrap().info.path))
            .expect("cannot remove test file");
    }

    #[test]
    fn should_read_piece_from_single_file() {
        let file_range = 0..1;
        let piece = make_piece(file_range.clone());
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_read_single_file.test"),
                torrent_offset: 0,
                len: 2 * piece.len as u64,
            },
        )
        .expect("cannot create test file");
        let files = &[sync::RwLock::new(file)];

        let torrent_piece_offset = 0;
        piece.write(torrent_piece_offset, files).expect("cannot write piece to file");

        // read piece as list of blocks
        let blocks = piece::read(torrent_piece_offset, file_range, files, piece.len)
            .expect("cannot read piece from file");

        // compare contents
        // map Vec<Arc<Vec<u8>>> to Vec<Vec<u8>>
        let actual: Vec<_> = blocks.iter().flat_map(AsRef::as_ref).cloned().collect();
        let expected: Vec<_> = piece.blocks.values().flatten().copied().collect();
        assert_eq!(actual, expected);

        // clean up env
        fs::remove_file(download_dir.join(&files[0].read().unwrap().info.path))
            .expect("cannot remove test file");
    }

    /// Tests that writing piece to multiple files works.
    #[test]
    fn should_write_piece_to_multiple_files() {
        // piece spans 3 files
        let file_range = 0..3;
        let piece = make_piece(file_range);
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file1 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files1.test"),
                torrent_offset: 0,
                len: BLOCK_LEN as u64 + 3,
            },
        )
        .expect("cannot create test file 1");
        let file2 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files2.test"),
                torrent_offset: file1.info.len,
                len: BLOCK_LEN as u64 - 1500,
            },
        )
        .expect("cannot create test file 2");
        let file3 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files3.test"),
                torrent_offset: file2.info.torrent_offset + file2.info.len,
                len: piece.len as u64 - (file1.info.len + file2.info.len),
            },
        )
        .expect("cannot create test file 3");
        let files = &[sync::RwLock::new(file1), sync::RwLock::new(file2), sync::RwLock::new(file3)];

        // piece starts at the beginning of files
        let torrent_piece_offset = 0;
        piece.write(torrent_piece_offset, files).expect("cannot write piece to file");

        // compare contents of files to piece
        for file in files.iter() {
            let mut file = file.write().unwrap();
            let mut file_content = Vec::new();
            file.handle.read_to_end(&mut file_content).expect("cannot read test file");
            // compare the content of file to the portion that corresponds to
            // piece
            assert_eq!(
                file_content,
                piece
                    .blocks
                    .values()
                    .flatten()
                    .cloned()
                    .skip(file.info.torrent_offset as usize)
                    .take(file.info.len as usize)
                    .collect::<Vec<_>>(),
                "file {:?} content does not equal piece",
                file.info
            );
        }

        // clean up env
        for file in files.iter() {
            let path = download_dir.join(&file.read().unwrap().info.path);
            fs::remove_file(path).expect("cannot remove test file");
        }
    }

    #[test]
    fn should_read_piece_from_multiple_files() {
        let file_range = 0..3;
        let piece = make_piece(file_range.clone());
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file1 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files1.test"),
                torrent_offset: 0,
                len: BLOCK_LEN as u64 + 3,
            },
        )
        .expect("cannot create test file 1");
        let file2 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files2.test"),
                torrent_offset: file1.info.len,
                len: BLOCK_LEN as u64 - 1500,
            },
        )
        .expect("cannot create test file 2");
        let file3 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files3.test"),
                torrent_offset: file2.info.torrent_offset + file2.info.len,
                len: piece.len as u64 - (file1.info.len + file2.info.len),
            },
        )
        .expect("cannot create test file 3");
        let files = &[sync::RwLock::new(file1), sync::RwLock::new(file2), sync::RwLock::new(file3)];

        // piece starts at the beginning of files
        let torrent_piece_offset = 0;
        piece.write(torrent_piece_offset, files).expect("cannot write piece to file");

        // read piece as list of blocks
        let blocks = piece::read(torrent_piece_offset, file_range, files, piece.len)
            .expect("cannot read piece from files");

        // compare contents
        // map Vec<Arc<Vec<u8>>> to Vec<Vec<u8>>
        let actual: Vec<_> = blocks.iter().flat_map(AsRef::as_ref).cloned().collect();
        let expected: Vec<_> = piece.blocks.values().flatten().copied().collect();
        assert_eq!(actual, expected);
    }

    /// Creates a piece for testing that has 4 blocks of length `BLOCK_LEN`.
    fn make_piece(files: Range<FileIndex>) -> Piece {
        let blocks = vec![
            (0..BLOCK_LEN).map(|b| b % u8::MAX as u32).map(|b| b as u8).collect::<Vec<u8>>(),
            (BLOCK_LEN..2 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
            (2 * BLOCK_LEN..3 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
            (3 * BLOCK_LEN..4 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
        ];
        let expected_hash = {
            let mut hasher = Sha1::new();
            for block in blocks.iter() {
                hasher.update(block);
            }
            hasher.finalize().as_slice().try_into().unwrap()
        };
        let len = blocks.len() as u32 * BLOCK_LEN;
        // convert blocks to a b-tree map
        let (blocks, _) =
            blocks.into_iter().fold((BTreeMap::new(), 0u32), |(mut map, mut offset), block| {
                let block_len = block.len();
                map.insert(offset, block);
                offset += block_len as u32;
                (map, offset)
            });
        Piece { expected_hash, len, blocks, file_range: files }
    }
}
