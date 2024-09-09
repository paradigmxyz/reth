use crate::{NippyJarError, OFFSETS_FILE_EXTENSION};
use memmap2::Mmap;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
};

#[cfg(windows)]
pub type DefaultDataReader = FileDataReader;

#[cfg(not(windows))]
pub type DefaultDataReader = MmapDataReader;

/// Trait defining shared methods for reading data.
pub trait DataReader {
    /// Reads the respective data and offsets file and returns [`DataReader`].
    fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError>
    where
        Self: Sized;

    /// Returns the offset for the requested data index.
    fn offset(&self, index: usize) -> Result<u64, NippyJarError>;

    /// Returns the offset for the requested data index starting from the end.
    fn reverse_offset(&self, index: usize) -> Result<u64, NippyJarError>;

    /// Returns the total number of offsets in the file.
    ///
    /// The size of one offset is determined by the file itself.
    fn offsets_count(&self) -> Result<usize, NippyJarError>;

    /// Reads one offset-sized (determined by the offset file) `u64` at the provided index.
    fn offset_at(&self, index: usize) -> Result<u64, NippyJarError>;

    /// Returns number of bytes that represent one offset.
    fn offset_size(&self) -> u8;

    /// Whether this data reader supports [`Self::data_ref`]
    fn allows_data_ref(&self) -> bool;

    /// Returns the underlying data range as a slice of bytes.
    fn data_ref(&self, range: Range<usize>) -> &[u8];

    /// Copies the underlying data range to a given mutable slice.
    fn data(&self, range: Range<usize>, buffer: &mut [u8]) -> Result<(), NippyJarError>;

    /// Returns the total size of the data.
    fn size(&self) -> Result<usize, NippyJarError>;
}

/// Manages the reading of static file data using memory-mapped files.
///
/// Holds file and mmap descriptors of the data and offsets files of a `static_file`.
#[derive(Debug)]
pub struct MmapDataReader {
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

impl DataReader for MmapDataReader {
    fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError> {
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

    fn offset(&self, index: usize) -> Result<u64, NippyJarError> {
        // + 1 represents the offset_len u8 which is in the beginning of the file
        let from = index * self.offset_size as usize + 1;

        self.offset_at(from)
    }

    fn reverse_offset(&self, index: usize) -> Result<u64, NippyJarError> {
        let offsets_file_size = self.offset_file.metadata()?.len() as usize;

        if offsets_file_size > 1 {
            let from = offsets_file_size - self.offset_size as usize * (index + 1);

            self.offset_at(from)
        } else {
            Ok(0)
        }
    }

    fn offsets_count(&self) -> Result<usize, NippyJarError> {
        Ok((self.offset_file.metadata()?.len().saturating_sub(1) / self.offset_size as u64)
            as usize)
    }

    fn offset_at(&self, index: usize) -> Result<u64, NippyJarError> {
        let mut buffer: [u8; 8] = [0; 8];

        let offset_end = index.saturating_add(self.offset_size as usize);
        if offset_end > self.offset_mmap.len() {
            return Err(NippyJarError::OffsetOutOfBounds { index })
        }

        buffer[..self.offset_size as usize].copy_from_slice(&self.offset_mmap[index..offset_end]);
        Ok(u64::from_le_bytes(buffer))
    }

    fn offset_size(&self) -> u8 {
        self.offset_size
    }

    fn allows_data_ref(&self) -> bool {
        true
    }

    fn data_ref(&self, range: Range<usize>) -> &[u8] {
        &self.data_mmap[range]
    }

    fn data(&self, range: Range<usize>, buffer: &mut [u8]) -> Result<(), NippyJarError> {
        buffer.copy_from_slice(&self.data_mmap[range]);
        Ok(())
    }

    fn size(&self) -> Result<usize, NippyJarError> {
        Ok(self.data_mmap.len())
    }
}

/// Manages the reading of static file data using direct file access.
#[derive(Debug)]
pub struct FileDataReader {
    /// Data file descriptor.
    data_file: File,
    /// Offset file descriptor.
    offset_file: File,
    /// Number of bytes that represent one offset.
    offset_size: u8,
}

impl DataReader for FileDataReader {
    fn new(path: impl AsRef<Path>) -> Result<Self, NippyJarError> {
        let data_file = File::open(path.as_ref())?;
        let mut offset_file = File::open(path.as_ref().with_extension(OFFSETS_FILE_EXTENSION))?;

        // First byte is the size of one offset in bytes
        let mut offset_size_buf = [0u8; 1];
        offset_file.read_exact(&mut offset_size_buf)?;
        let offset_size = offset_size_buf[0];

        if offset_size > 8 {
            return Err(NippyJarError::OffsetSizeTooBig { offset_size });
        } else if offset_size == 0 {
            return Err(NippyJarError::OffsetSizeTooSmall { offset_size });
        }

        Ok(Self { data_file, offset_file, offset_size })
    }

    fn offset(&self, index: usize) -> Result<u64, NippyJarError> {
        let from = index * self.offset_size as usize + 1;
        self.offset_at(from)
    }

    fn reverse_offset(&self, index: usize) -> Result<u64, NippyJarError> {
        let offsets_file_size = self.offset_file.metadata()?.len() as usize;
        if offsets_file_size > 1 {
            let from = offsets_file_size - self.offset_size as usize * (index + 1);
            self.offset_at(from)
        } else {
            Ok(0)
        }
    }

    fn offsets_count(&self) -> Result<usize, NippyJarError> {
        Ok((self.offset_file.metadata()?.len().saturating_sub(1) / self.offset_size as u64)
            as usize)
    }

    fn offset_at(&self, index: usize) -> Result<u64, NippyJarError> {
        let mut buffer: [u8; 8] = [0; 8];
        let mut offset_file = &self.offset_file;
        offset_file.seek(SeekFrom::Start(index as u64))?;
        offset_file.read_exact(&mut buffer[..self.offset_size as usize])?;
        Ok(u64::from_le_bytes(buffer))
    }

    fn offset_size(&self) -> u8 {
        self.offset_size
    }

    fn allows_data_ref(&self) -> bool {
        false
    }

    fn data_ref(&self, _: Range<usize>) -> &[u8] {
        unimplemented!()
    }

    fn data(&self, range: Range<usize>, buffer: &mut [u8]) -> Result<(), NippyJarError> {
        debug_assert!(range.len() == buffer.len());

        let mut data_file = &self.offset_file;
        data_file.seek(SeekFrom::Start(range.start as u64))?;
        data_file.read_exact(buffer)?;
        Ok(())
    }

    fn size(&self) -> Result<usize, NippyJarError> {
        Ok(self.data_file.metadata()?.len() as usize)
    }
}
