use crate::{
    disk::error::{NewTorrentError, ReadError, WriteError},
    info::{storage::FileSlice, FileInfo},
};
use std::{
    fs::{File, OpenOptions},
    os::unix::io::AsRawFd,
    path::Path,
};
use tracing::{trace, warn};

pub(crate) struct TorrentFile {
    pub(crate) info: FileInfo,
    pub(crate) handle: File,
}

impl TorrentFile {
    /// Opens the file in create, read, and write modes at the path of combining the
    /// download directory and the path defined in the file info.
    pub(crate) fn new(download_dir: &Path, info: FileInfo) -> Result<Self, NewTorrentError> {
        trace!("Opening and creating file {:?} in dir {:?}", info, download_dir);
        let path = download_dir.join(&info.path);
        let handle =
            OpenOptions::new().create(true).write(true).read(true).open(&path).map_err(|e| {
                warn!("Failed to open file {:?}", path);
                NewTorrentError::Io(e)
            })?;
        Ok(Self { info, handle })
    }

    // /// Writes to file at most the slice length number of bytes of blocks at the
    // /// file slice's offset, using pwritev, called repeteadly until all blocks are
    // /// written to disk.
    // ///
    // /// It returns the slice of blocks that weren't written to disk. That is, it
    // /// returns the second half of `blocks` as though they were split at the
    // /// `file_slice.len` offset. If all blocks were written to disk an empty
    // /// slice is returned.
    // ///
    // /// # Important
    // ///
    // /// Since the syscall may be invoked repeatedly to perform disk IO, this
    // /// means that this operation is not guaranteed to be atomic.
    // pub fn write<'a>(
    //     &self,
    //     file_slice: FileSlice,
    //     blocks: &'a mut [IoVec<&'a [u8]>],
    // ) -> Result<&'a mut [IoVec<&'a [u8]>], WriteError> {
    //     let mut iovecs = IoVecs::bounded(blocks, file_slice.len as usize);
    //     // the write buffer cannot be larger than the file slice we want to
    //     // write to
    //     debug_assert!(
    //         iovecs.as_slice().iter().map(|iov| iov.as_slice().len() as u64).sum::<u64>() <=
    //             file_slice.len
    //     );
    //
    //     // IO syscalls are not guaranteed to transfer the whole input buffer in one
    //     // go, so we need to repeat until all bytes have been confirmed to be
    //     // transferred to disk (or an error occurs)
    //     let mut total_write_count = 0;
    //     while !iovecs.as_slice().is_empty() {
    //         let write_count =
    //             pwritev(self.handle.as_raw_fd(), iovecs.as_slice(), file_slice.offset as i64)
    //                 .map_err(|e| {
    //                     warn!("File {:?} write error: {}", self.info.path, e);
    //                     // FIXME: convert actual error here
    //                     WriteError::Io(std::io::Error::last_os_error())
    //                 })?;
    //
    //         // tally up the total write count
    //         total_write_count += write_count;
    //
    //         // no need to advance write buffers cursor if we've written
    //         // all of it to file--in that case, we can just split the iovecs
    //         // and return the second half, consuming the first half
    //         if total_write_count as u64 == file_slice.len {
    //             break
    //         }
    //
    //         // advance the buffer cursor in iovecs by the number of bytes
    //         // transferred
    //         iovecs.advance(write_count);
    //     }
    //
    //     Ok(iovecs.into_tail())
    // }

    // /// Reads from file at most the slice length number of bytes of blocks at
    // /// the file slice's offset, using preadv, called repeteadly until all
    // /// blocks are read from disk.
    // ///
    // /// It returns the slice of block buffers that weren't filled by the
    // /// disk-read. That is, it returns the second half of `blocks` as though
    // /// they were split at the `file_slice.len` offset. If all blocks were read
    // /// from disk an empty slice is returned.
    // ///
    // /// # Important
    // ///
    // /// Since the syscall may be invoked repeatedly to perform disk IO, this
    // /// means that this operation is not guaranteed to be atomic.
    // pub fn read<'a>(
    //     &self,
    //     file_slice: FileSlice,
    //     mut iovecs: &'a mut [IoVec<&'a mut [u8]>],
    // ) -> Result<&'a mut [IoVec<&'a mut [u8]>], ReadError> {
    //     // This is simpler than the write implementation as the preadv method
    //     // stops reading in from the file if reaching EOF. We do need to advance
    //     // the iovecs read buffer cursor after a read as we may want to read
    //     // from other files after this one, in which case the cursor should
    //     // be on the next byte to read to.
    //
    //     // IO syscalls are not guaranteed to transfer the whole input buffer in one
    //     // go, so we need to repeat until all bytes have been confirmed to be
    //     // transferred to disk (or an error occurs)
    //     let mut total_read_count = 0;
    //     while !iovecs.is_empty() && (total_read_count as u64) < file_slice.len {
    //         let read_count = preadv(
    //             self.handle.as_raw_fd(),
    //             iovecs,
    //             file_slice.offset as i64,
    //         )
    //         .map_err(|e| {
    //             warn!("File {:?} read error: {}", self.info.path, e);
    //             // FIXME: convert actual error here
    //             ReadError::Io(std::io::Error::last_os_error())
    //         })?;
    //
    //         // if there was nothing to read from file it means we tried to
    //         // read a piece from a portion of a file not yet downloaded or
    //         // otherwise missing
    //         if read_count == 0 {
    //             return Err(ReadError::MissingData);
    //         }
    //
    //         // tally up the total read count
    //         total_read_count += read_count;
    //
    //         // advance the buffer cursor in iovecs by the number of bytes
    //         // transferred
    //         iovecs = iovecs::advance(iovecs, read_count);
    //     }
    //
    //     Ok(iovecs)
    // }
}
