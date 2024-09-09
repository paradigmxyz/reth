use std::ops::Range;
use std::{fs::File, path::Path};
use memmap2::Mmap;
use crate::{NippyJarError, OFFSETS_FILE_EXTENSION};


/// Manages the reading of static file data using memory-mapped files.
///
/// Holds file and mmap descriptors of the data and offsets files of a `static_file`.
#[derive(Debug)]
pub struct DataReader {
    /// Data file descriptor. Needs to be kept alive as long as `data_mmap` handle.
    #[allow(dead_code)]
    data_file: File,
    /// Mmap handle for data.
    data_mmap: Mmap,
    /// Offset file descriptor. Needs to be kept alive as long as `offset_mmap` handle.
    #[allow(dead_code)]
    offset_file: File,
    /// Mmap handle for offsets.
    offset_mmap: Mmap,
    /// Number of bytes that represent one offset.
    offset_size: u8,
}

impl DataReader {
    /// Reads the respective data and offsets file and returns [`DataReader`].
    pub fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError> {
        let data_file = File::open(path.as_ref())?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let data_mmap = unsafe { Mmap::map(&data_file)? };

        let offset_file = File::open(path.as_ref().with_extension(OFFSETS_FILE_EXTENSION))?;
        // SAFETY: File is read-only and its descriptor is kept alive as long as the mmap handle.
        let offset_mmap = unsafe { Mmap::map(&offset_file)? };

        // First byte is the size of one offset in bytes
        let offset_size = offset_mmap[0];

        // Ensure that the size of an offset is at most 8 bytes.
        if offset_size > 8 {
            return Err(NippyJarError::OffsetSizeTooBig { offset_size })
        } else if offset_size == 0 {
            return Err(NippyJarError::OffsetSizeTooSmall { offset_size })
        }

        Ok(Self { data_file, data_mmap, offset_file, offset_size, offset_mmap })
    }

    /// Returns the offset for the requested data index
    pub fn offset(&self, index: usize) -> Result<u64, NippyJarError> {
        // + 1 represents the offset_len u8 which is in the beginning of the file
        let from = index * self.offset_size as usize + 1;

        self.offset_at(from)
    }

    /// Returns the offset for the requested data index starting from the end
    pub fn reverse_offset(&self, index: usize) -> Result<u64, NippyJarError> {
        let offsets_file_size = self.offset_file.metadata()?.len() as usize;

        if offsets_file_size > 1 {
            let from = offsets_file_size - self.offset_size as usize * (index + 1);

            self.offset_at(from)
        } else {
            Ok(0)
        }
    }

    /// Returns total number of offsets in the file.
    /// The size of one offset is determined by the file itself.
    pub fn offsets_count(&self) -> Result<usize, NippyJarError> {
        Ok((self.offset_file.metadata()?.len().saturating_sub(1) / self.offset_size as u64)
            as usize)
    }

    /// Reads one offset-sized (determined by the offset file) u64 at the provided index.
    fn offset_at(&self, index: usize) -> Result<u64, NippyJarError> {
        let mut buffer: [u8; 8] = [0; 8];

        let offset_end = index.saturating_add(self.offset_size as usize);
        if offset_end > self.offset_mmap.len() {
            return Err(NippyJarError::OffsetOutOfBounds { index })
        }

        buffer[..self.offset_size as usize].copy_from_slice(&self.offset_mmap[index..offset_end]);
        Ok(u64::from_le_bytes(buffer))
    }

    /// Returns number of bytes that represent one offset.
    pub const fn offset_size(&self) -> u8 {
        self.offset_size
    }

    /// Returns the underlying data as a slice of bytes for the provided range.
    pub fn data(&self, range: Range<usize>) -> &[u8] {
        &self.data_mmap[range]
    }

    /// Returns total size of data
    pub fn size(&self) -> usize {
        self.data_mmap.len()
    }
}
