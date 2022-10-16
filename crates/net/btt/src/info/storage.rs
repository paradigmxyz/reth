use crate::info::{meta::Metainfo, FileIndex, PieceIndex};
use std::{ops::Range, path::PathBuf};

/// Information about a torrent's file.
#[derive(Clone, Debug)]
pub struct FileInfo {
    /// The file's relative path from the download directory.
    pub path: PathBuf,
    /// The file's length, in bytes.
    pub len: u64,
    /// The byte offset of the file within the torrent, when all files in
    /// torrent are viewed as a single contiguous byte array. This is always
    /// 0 for a single file torrent.
    pub torrent_offset: u64,
}

impl FileInfo {
    /// Returns a range that represents the file's first and one past the last
    /// bytes' offsets in the torrent.
    pub fn byte_range(&self) -> Range<u64> {
        self.torrent_offset..self.torrent_end_offset()
    }

    /// Returns the file's one past the last byte's offset in the torrent.
    pub fn torrent_end_offset(&self) -> u64 {
        self.torrent_offset + self.len
    }

    /// Returns the slice in file that overlaps with the range starting at the
    /// given offset.
    ///
    /// # Arguments
    ///
    /// * `torrent_offset` - A byte offset in the entire torrent.
    /// * `len` - The length of the byte range, starting from the offset. This may exceed the file
    ///   length, in which case the returned file length will be smaller.
    ///
    /// # Panics
    ///
    /// This will panic if `torrent_offset` is smaller than the file's offset in
    /// torrent, or if it's past the last byte in file.
    pub fn get_slice(&self, torrent_offset: u64, len: u64) -> FileSlice {
        assert!(
            torrent_offset >= self.torrent_offset,
            "torrent offset must be larger than file offset",
        );

        let torrent_end_offset = self.torrent_end_offset();
        assert!(
            torrent_offset < torrent_end_offset,
            "torrent offset must be smaller than file end offset",
        );

        FileSlice {
            offset: torrent_offset - self.torrent_offset,
            len: len.min(torrent_end_offset - torrent_offset),
        }
    }
}

/// Represents the location of a range of bytes within a file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileSlice {
    /// The byte offset in file, relative to the file's start.
    pub offset: u64,
    /// The length of the slice, in bytes.
    pub len: u64,
}

/// Information about a torrent's storage details, such as the piece count and
/// length, download length, etc.
#[derive(Clone, Debug)]
pub(crate) struct StorageInfo {
    /// The number of pieces in the torrent.
    pub(crate) piece_count: usize,
    /// The nominal length of a piece.
    pub(crate) piece_len: u32,
    /// The length of the last piece in torrent, which may differ from the
    /// normal piece length if the download size is not an exact multiple of the
    /// piece length.
    pub(crate) last_piece_len: u32,
    /// The sum of the length of all files in the torrent.
    pub(crate) download_len: u64,
    /// The download destination directory of the torrent.
    ///
    /// In case of single file downloads, this is the directory where the file
    /// is downloaded, named as the torrent.
    /// In case of archive downloads, this directory is the download directory
    /// joined by the torrent's name. This is because in case of a torrent that
    /// has multiple top-level entries, the downloaded files would be scattered
    /// across the download directory, which is an annoyance we want to avoid.
    /// E.g. downloading files into ~/Downloads/<torrent> instead of just
    /// ~/Downloads.
    pub(crate) download_dir: PathBuf,
    /// All files in torrent.
    pub(crate) files: Vec<FileInfo>,
}

impl StorageInfo {
    /// Extracts storage related information from the torrent metainfo.
    pub(crate) fn new(metainfo: &Metainfo, download_dir: PathBuf) -> Self {
        let piece_count = metainfo.piece_count();
        let download_len = metainfo.download_len();
        let piece_len = metainfo.piece_len;
        let last_piece_len = download_len - piece_len as u64 * (piece_count - 1) as u64;
        let last_piece_len = last_piece_len as u32;

        // if this is an archive, download files into torrent's own dir
        let download_dir =
            if metainfo.is_archive() { download_dir.join(&metainfo.name) } else { download_dir };

        Self {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_dir,
            files: metainfo.files.clone(),
        }
    }

    /// Returns the zero-based indices of the files of torrent that intersect
    /// with the piece.
    ///
    /// # Panics
    ///
    /// Panics if the piece index is invalid. Validation must happen at the
    /// protocol level. The internals of the engine work on the assumption that
    /// piece indices are valid.
    pub(crate) fn files_intersecting_piece(&self, index: PieceIndex) -> Range<FileIndex> {
        let piece_offset = index as u64 * self.piece_len as u64;
        let piece_end = piece_offset + self.piece_len(index) as u64;
        self.files_intersecting_bytes(piece_offset..piece_end)
    }

    /// Returns the files that overlap with the given left-inclusive range of
    /// bytes, where `bytes.start` is the offset and `bytes.end` is one past the
    /// last byte offset.
    pub(crate) fn files_intersecting_bytes(&self, byte_range: Range<u64>) -> Range<FileIndex> {
        debug_assert_ne!(self.files.len(), 0);
        if self.files.len() == 1 {
            // when torrent only has one file, only that file can be returned
            0..1
        } else {
            // find the index of the first file that contains the first byte
            // of the range
            let first_matching_index = match self
                .files
                .iter()
                .enumerate()
                .find(|(_, file)| {
                    // check if the file's byte range contains the first
                    // byte of the range
                    file.byte_range().contains(&byte_range.start)
                })
                .map(|(index, _)| index)
            {
                Some(index) => index,
                None => return 0..0,
            };

            // the resulting files
            let mut file_range = first_matching_index..first_matching_index + 1;

            // Find the the last file that contains the last byte of the
            // range, starting at the file after the above found one.
            //
            // NOTE: the order of `enumerate` and `skip` matters as
            // otherwise we'd be getting relative indices
            for (index, file) in self.files.iter().enumerate().skip(first_matching_index + 1) {
                // stop if file's first byte is not contained by the byte
                // range (is at or past the end of the byte range we're
                // looking for)
                if !byte_range.contains(&file.torrent_offset) {
                    break
                }

                // note that we need to add one to the end as this is
                // a left-inclusive range, so we want the end (excluded) to
                // be one past the actually included value
                file_range.end = index + 1;
            }

            file_range
        }
    }

    /// Returns the piece's absolute offset in the torrent.
    pub(crate) fn torrent_piece_offset(&self, index: PieceIndex) -> u64 {
        index as u64 * self.piece_len as u64
    }

    /// Returns the length of the piece at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the piece index is invalid. Validation must happen at the
    /// protocol level. The internals of the engine work on the assumption that
    /// piece indices are valid.
    pub(crate) fn piece_len(&self, index: PieceIndex) -> u32 {
        assert!(index < self.piece_count, "piece index out of range");
        if index == self.piece_count - 1 {
            self.last_piece_len
        } else {
            self.piece_len
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_get_slice() {
        let file = FileInfo {
            // file doesn't need to exist as we're not doing any IO in this test
            path: PathBuf::from("/tmp/does/not/exist"),
            len: 500,
            torrent_offset: 200,
        };

        assert_eq!(
            file.get_slice(300, 1000),
            FileSlice { offset: 300 - 200, len: 500 - (300 - 200) },
            "file slice for byte range longer than file should return \
            at most file length long slice"
        );

        assert_eq!(
            file.get_slice(300, 10),
            FileSlice { offset: 300 - 200, len: 10 },
            "file slice for byte range smaller than file should return \
            at most byte range long slice"
        );

        assert_eq!(
            file.get_slice(200, 500),
            FileSlice { offset: 0, len: 500 },
            "file slice for byte range equal to file length should return \
            the full file slice"
        );
    }

    #[test]
    #[should_panic(expected = "torrent offset must be larger than file offset")]
    fn test_file_get_slice_starting_before_file() {
        let file = FileInfo {
            // file doesn't need to exist as we're not doing any IO in this test
            path: PathBuf::from("/tmp/does/not/exist"),
            len: 500,
            torrent_offset: 200,
        };
        // we can't query a file slace for a byte range starting before the file
        file.get_slice(100, 400);
    }

    #[test]
    #[should_panic(expected = "torrent offset must be smaller than file end offset")]
    fn test_file_get_slice_starting_after_file() {
        let file = FileInfo {
            // file doesn't need to exist as we're not doing any IO in this test
            path: PathBuf::from("/tmp/does/not/exist"),
            len: 500,
            torrent_offset: 200,
        };
        // we can't query a file slace for a byte range starting before the file
        file.get_slice(200 + 500, 400);
    }

    #[test]
    fn test_files_intersecting_pieces() {
        // single file
        let piece_count = 4;
        let piece_len = 4;
        let last_piece_len = 2;
        // 3 full length pieces; 1 smaller piece,
        let download_len = 3 * 4 + 2;
        let files =
            vec![FileInfo { path: PathBuf::from("/bogus"), torrent_offset: 0, len: download_len }];
        let info = StorageInfo {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_dir: PathBuf::from("/"),
            files,
        };
        // all 4 pieces are in the same file
        assert_eq!(info.files_intersecting_piece(0), 0..1);
        assert_eq!(info.files_intersecting_piece(1), 0..1);
        assert_eq!(info.files_intersecting_piece(2), 0..1);
        assert_eq!(info.files_intersecting_piece(3), 0..1);

        // multi-file
        //
        // pieces: (index:first byte offset)
        // --------------------------------------------------------------------
        // |0:0         |1:16          |2:32          |3:48          |4:64    |
        // --------------------------------------------------------------------
        // files: (index:first byte offset,last byte offset)
        // --------------------------------------------------------------------
        // |0:0,8 |1:9,19  |2:20,26|3:27,35 |4:36,47  |5:48,63       |6:64,71 |
        // --------------------------------------------------------------------
        let files = vec![
            FileInfo { path: PathBuf::from("/0"), torrent_offset: 0, len: 9 },
            FileInfo { path: PathBuf::from("/1"), torrent_offset: 9, len: 11 },
            FileInfo { path: PathBuf::from("/2"), torrent_offset: 20, len: 7 },
            FileInfo { path: PathBuf::from("/3"), torrent_offset: 27, len: 9 },
            FileInfo { path: PathBuf::from("/4"), torrent_offset: 36, len: 12 },
            FileInfo { path: PathBuf::from("/5"), torrent_offset: 48, len: 16 },
            FileInfo { path: PathBuf::from("/6"), torrent_offset: 64, len: 8 },
        ];
        let download_len: u64 = files.iter().map(|f| f.len).sum();
        // sanity check that the offsets in the files above correctly follow
        // each other and that they add up to the total download length
        debug_assert_eq!(
            files.iter().fold(0, |offset, file| {
                debug_assert_eq!(offset, file.torrent_offset);
                offset + file.len
            }),
            download_len,
        );
        let piece_count: usize = 5;
        let piece_len: u32 = 16;
        let last_piece_len: u32 = 8;
        // sanity check that full piece lengths and last piece length equals the
        // total download length
        debug_assert_eq!(
            (piece_count as u64 - 1) * piece_len as u64 + last_piece_len as u64,
            download_len
        );
        let info = StorageInfo {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_dir: PathBuf::from("/"),
            files,
        };
        // piece 0 intersects with files 0 and 1
        assert_eq!(info.files_intersecting_piece(0), 0..2);
        // piece 1 intersects with files 1, 2, 3
        assert_eq!(info.files_intersecting_piece(1), 1..4);
        // piece 2 intersects with files 3 and 4
        assert_eq!(info.files_intersecting_piece(2), 3..5);
        // piece 3 intersects with only file 5
        assert_eq!(info.files_intersecting_piece(3), 5..6);
        // last piece 4 intersects with only file 6
        assert_eq!(info.files_intersecting_piece(4), 6..7);
    }

    #[test]
    fn test_files_intersecting_bytes() {
        let download_len = 12341234;
        let files =
            vec![FileInfo { path: PathBuf::from("/bogus"), torrent_offset: 0, len: download_len }];
        let info = StorageInfo {
            // arbitrary piece info (not used in this test)
            piece_count: 4,
            piece_len: 4,
            last_piece_len: 2,
            download_len,
            download_dir: PathBuf::from("/"),
            files,
        };
        assert_eq!(info.files_intersecting_bytes(0..0), 0..1);
        assert_eq!(info.files_intersecting_bytes(0..1), 0..1);
        assert_eq!(info.files_intersecting_bytes(0..12341234), 0..1);

        // multi-file
        let files = vec![
            FileInfo { path: PathBuf::from("/bogus0"), torrent_offset: 0, len: 4 },
            FileInfo { path: PathBuf::from("/bogus1"), torrent_offset: 4, len: 9 },
            FileInfo { path: PathBuf::from("/bogus2"), torrent_offset: 13, len: 3 },
            FileInfo { path: PathBuf::from("/bogus3"), torrent_offset: 16, len: 10 },
        ];
        let download_len = files.iter().map(|f| f.len).sum();
        let info = StorageInfo {
            // arbitrary piece info (not used in this test)
            piece_count: 4,
            piece_len: 4,
            last_piece_len: 2,
            download_len,
            download_dir: PathBuf::from("/"),
            files,
        };

        // bytes only in the first file
        assert_eq!(info.files_intersecting_bytes(0..4), 0..1);
        // bytes intersecting two files
        assert_eq!(info.files_intersecting_bytes(0..5), 0..2);
        // bytes overlapping with two files
        assert_eq!(info.files_intersecting_bytes(0..13), 0..2);
        // bytes intersecting three files
        assert_eq!(info.files_intersecting_bytes(0..15), 0..3);
        // bytes intersecting all files
        assert_eq!(info.files_intersecting_bytes(0..18), 0..4);
        // bytes intersecting the last byte of the last file
        assert_eq!(info.files_intersecting_bytes(25..26), 3..4);
        // bytes overlapping with two files in the middle
        assert_eq!(info.files_intersecting_bytes(4..16), 1..3);
        // bytes intersecting only one byte of two files each, among the middle
        // of all files
        assert_eq!(info.files_intersecting_bytes(8..14), 1..3);
        // bytes intersecting only one byte of one file, among the middle of all
        // files
        assert_eq!(info.files_intersecting_bytes(13..14), 2..3);
        // bytes not intersecting any files
        assert_eq!(info.files_intersecting_bytes(30..38), 0..0);
    }
}
