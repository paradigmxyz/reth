use crate::{compression::Compression, ColumnResult, NippyJar, NippyJarError};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Writer of [`NippyJar`]. Handles table data and offsets only. 
/// 
/// Table data is written directly to disk, while offsets and configuration need to be flushed by calling `commit()`.
pub struct NippyJarWriter<'a, H> {
    /// Reference to the associated [`NippyJar`], containing all necessary configurations for data handling.
    jar: &'a mut NippyJar<H>,
    /// File handle to where the data is stored.
    data_file: File,
    /// File handle to where the offsets are stored.
    offsets_file: File,
    /// Temporary buffer to reuse when compressing data.
    tmp_buf: Vec<u8>,
    /// Used to find the maximum uncompressed size of a row in a jar.
    uncompressed_row_size: usize,
    /// Partial offset list which hasn't been flushed to disk.
    offsets: Vec<u64>,
    /// Column where writer is going to write next.
    column: usize,
}

impl<'a, H> NippyJarWriter<'a, H>
where
    H: Send + Sync + Serialize + for<'b> Deserialize<'b> + std::fmt::Debug,
{
    pub fn new(jar: &'a mut NippyJar<H>) -> Result<Self, NippyJarError> {
        let (data_file, offsets_file) =
            Self::create_or_open_files(jar.data_path(), &jar.offsets_path())?;

        let mut writer = Self {
            jar,
            data_file,
            offsets_file,
            tmp_buf: Vec::with_capacity(1_000_000),
            uncompressed_row_size: 0,
            offsets: Vec::with_capacity(1_000_000),
            column: 0,
        };

        Ok(writer)
    }

    fn create_or_open_files(data: &Path, offsets: &Path) -> Result<(File, File), NippyJarError> {
        let mut data_file = if !data.exists() {
            File::create(data)?
        } else {
            OpenOptions::new().read(true).write(true).open(data)?
        };
        data_file.seek(SeekFrom::End(0))?;

        let mut offsets_file = if !offsets.exists() {
            let mut offsets = File::create(offsets)?;

            // Initially all offsets are represented as 8 bytes.
            offsets.write_all(&[8])?;

            offsets
        } else {
            OpenOptions::new().read(true).write(true).open(offsets)?
        };
        offsets_file.seek(SeekFrom::End(0))?;

        Ok((data_file, offsets_file))
    }

    /// Appends rows to data file.
    pub fn append_rows(
        &mut self,
        columns: Vec<impl IntoIterator<Item = ColumnResult<Vec<u8>>>>,
        num_rows: u64,
    ) -> Result<(), NippyJarError> {
        let mut column_iterators =
            columns.into_iter().map(|v| v.into_iter()).collect::<Vec<_>>().into_iter();

        for _ in 0..num_rows {
            let mut iterators = Vec::with_capacity(self.jar.columns);

            for (column_number, mut column_iter) in column_iterators.enumerate() {
                self.offsets.push(self.data_file.stream_position()?);
                self.append_row(&mut column_iter, column_number)?;

                iterators.push(column_iter);
            }
            column_iterators = iterators.into_iter();
        }

        Ok(())
    }

    /// Appends a row to data file.
    pub fn append_row(
        &mut self,
        column_iter: &mut impl Iterator<Item = ColumnResult<Vec<u8>>>,
        column_number: usize,
    ) -> Result<(), NippyJarError> {
        match column_iter.next() {
            Some(Ok(value)) => {
                self.append_column(&value)?;
            }
            None => {
                return Err(NippyJarError::UnexpectedMissingValue(
                    self.jar.rows as u64,
                    column_number as u64,
                ))
            }
            Some(Err(err)) => return Err(err.into()),
        }

        Ok(())
    }

    /// Appends a column to data file.
    fn append_column(&mut self, value: &[u8]) -> Result<(), NippyJarError> {
        self.uncompressed_row_size += value.len();
        if let Some(compression) = &self.jar.compressor {
            let before = self.tmp_buf.len();
            let len = compression.compress_to(&value, &mut self.tmp_buf)?;
            self.data_file.write_all(&self.tmp_buf[before..before + len])?;
        } else {
            self.data_file.write_all(&value)?;
        }

        self.column += 1;

        if self.jar.columns == self.column {
            self.finalize_row();
        }

        Ok(())
    }

    /// Prunes rows from data file
    pub fn prune_rows(&mut self, number: usize) -> Result<(), NippyJarError> {
        // Calculate the number of offsets to prune from in-memory list
        let prune_count = number.min(self.offsets.len());
        let remaining_to_prune = number.saturating_sub(prune_count);

        // Prune in-memory offsets if needed
        if prune_count > 0 {
            // Determine new length based on the offset to prune up to
            let new_len = self.offsets[self.offsets.len() - prune_count];
            self.offsets.truncate(self.offsets.len() - prune_count);

            // Truncate the data file to the new length
            self.data_file.set_len(new_len)?;
        }

        // Prune from on-disk offset list if there are still rows left to prune
        if remaining_to_prune > 0 {
            // Get the current length of the on-disk offset file
            let length = self.offsets_file.metadata()?.len();
            let bytes_per_offset = 8;

            // Handle non-empty offset file
            if length > 1 {
                let num_offsets = (length - 1) / bytes_per_offset;
                let new_num_offsets = num_offsets.saturating_sub(remaining_to_prune as u64);

                // If all rows are to be pruned
                if new_num_offsets == 0 {
                    self.offsets_file.set_len(1)?;
                    self.data_file.set_len(0)?;
                } else {
                    // Calculate the new length for the on-disk offset list
                    let new_len = 1 + new_num_offsets * bytes_per_offset;
                    // Seek to the position of the last offset
                    self.offsets_file
                        .seek(SeekFrom::Start(new_len.saturating_sub(bytes_per_offset)))?;
                    // Read the last offset value
                    let mut last_offset = [0u8; 8];
                    self.offsets_file.read_exact(&mut last_offset)?;
                    let last_offset = u64::from_le_bytes(last_offset);

                    // Update the lengths of both the offsets and data files
                    self.offsets_file.set_len(new_len)?;
                    self.data_file.set_len(last_offset)?;
                }
            } else {
                // If the offsets file only contains the byte representation
                self.offsets_file.set_len(1)?;
                self.data_file.set_len(0)?;
            }
        }

        Ok(())
    }

    /// Updates [`NippyJar`] with the new row count and maximum uncompressed row size, while
    /// resetting internal fields.
    fn finalize_row(&mut self) {
        self.jar.max_row_size = self.jar.max_row_size.max(self.uncompressed_row_size);
        self.jar.rows += 1;

        self.tmp_buf.clear();
        self.uncompressed_row_size = 0;
        self.column = 0;
    }

    /// Commits configuration and offsets to disk. It drains the internal offset list.
    pub fn commit(&mut self) -> Result<(), NippyJarError> {
        // Appends new offsets to disk
        for offset in self.offsets.drain(..) {
            self.offsets_file.write_all(&offset.to_le_bytes())?;
        }

        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;

        Ok(())
    }
}
