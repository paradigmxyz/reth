use crate::{compression::Compression, ColumnResult, NippyJar, NippyJarError};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Writer of [`NippyJar`]. Handles table data and offsets only.
///
/// Table data is written directly to disk, while offsets and configuration need to be flushed by
/// calling `commit()`.
pub struct NippyJarWriter<'a, H> {
    /// Reference to the associated [`NippyJar`], containing all necessary configurations for data
    /// handling.
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
        let (data_file, offsets_file, is_created) =
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

        // If we are opening a previously created jar, we need to check its consistency, and make
        // changes if necessary.
        if !is_created {
            writer.consistency_check()?;
        }

        Ok(writer)
    }

    fn create_or_open_files(
        data: &Path,
        offsets: &Path,
    ) -> Result<(File, File, bool), NippyJarError> {
        let is_created = !data.exists() || !offsets.exists();

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

        Ok((data_file, offsets_file, is_created))
    }

    /// Performs consistency checks on the [`NippyJar`] file and acts upon any issues:
    /// * Is the offsets file size expected? If not, truncate it.
    /// * Is the data file size expected? If not truncate it.
    ///
    /// This is based on the assumption that [`NippyJar`] configuration is **always** the last one
    /// to be updated when something is written, as by the `commit()` function shows.
    fn consistency_check(&mut self) -> Result<(), NippyJarError> {
        let config_rows = self.jar.rows as u64;

        // 1 byte: for byte-len offset representation
        // 8 bytes * num rows * num columns
        // 8 bytes: expected size of the data file.
        let expected_offsets_file_size = 1 + config_rows * self.jar.columns as u64 * 8 + 8;
        let offsets_file_size = self.offsets_file.metadata()?.len();

        if expected_offsets_file_size != offsets_file_size {
            // TODO: ideally we could truncate until the last offset of the last column of the last
            // row inserted
            self.offsets_file.set_len(expected_offsets_file_size)?;
        }

        // Last offset is always the expected size of the data file.
        let mut last_offset = [0u8; 8];
        self.offsets_file.seek(SeekFrom::Start(1 + (config_rows * self.jar.columns as u64 * 8)))?;
        self.offsets_file.read_exact(&mut last_offset)?;
        let last_offset = u64::from_le_bytes(last_offset);

        // Offset list wasn't properly committed, so we need to truncate the data, since there's no
        // way to recover it.
        if last_offset != self.data_file.metadata()?.len() {
            self.data_file.set_len(last_offset)?;
        }

        self.offsets_file.seek(SeekFrom::End(0))?;
        self.data_file.seek(SeekFrom::End(0))?;

        Ok(())
    }

    /// Appends rows to data file.  `fn commit()` should be called to flush offsets and config to
    /// disk.
    ///
    /// `column_values_per_row`: A vector where each element is a column's values in sequence,
    /// corresponding to each row. The vector's length equals the number of columns.
    pub fn append_rows(
        &mut self,
        column_values_per_row: Vec<impl IntoIterator<Item = ColumnResult<impl AsRef<[u8]>>>>,
        num_rows: u64,
    ) -> Result<(), NippyJarError> {
        let mut column_iterators = column_values_per_row
            .into_iter()
            .map(|v| v.into_iter())
            .collect::<Vec<_>>()
            .into_iter();

        for _ in 0..num_rows {
            let mut iterators = Vec::with_capacity(self.jar.columns);

            for mut column_iter in column_iterators {
                self.append_column(column_iter.next())?;

                iterators.push(column_iter);
            }

            column_iterators = iterators.into_iter();
        }

        Ok(())
    }

    /// Appends a column to data file. `fn commit()` should be called to flush offsets and config to
    /// disk.
    pub fn append_column(
        &mut self,
        column: Option<ColumnResult<impl AsRef<[u8]>>>,
    ) -> Result<(), NippyJarError> {
        match column {
            Some(Ok(value)) => {
                if self.offsets.is_empty() {
                    // Represents the offset of the soon to be appended data column
                    self.offsets.push(self.data_file.stream_position()?);
                }

                self.write_column(value.as_ref())?;

                // Last offset represents the size of the data file if no more data is to be
                // appended. Otherwise, represents the offset of the next data item.
                self.offsets.push(self.data_file.stream_position()?);
            }
            None => {
                return Err(NippyJarError::UnexpectedMissingValue(
                    self.jar.rows as u64,
                    self.column as u64,
                ))
            }
            Some(Err(err)) => return Err(err.into()),
        }

        Ok(())
    }

    /// Writes column to data file. If it's the last column of the row, call `finalize_row()`
    fn write_column(&mut self, value: &[u8]) -> Result<(), NippyJarError> {
        self.uncompressed_row_size += value.len();
        if let Some(compression) = &self.jar.compressor {
            let before = self.tmp_buf.len();
            let len = compression.compress_to(value, &mut self.tmp_buf)?;
            self.data_file.write_all(&self.tmp_buf[before..before + len])?;
        } else {
            self.data_file.write_all(value)?;
        }

        self.column += 1;

        if self.jar.columns == self.column {
            self.finalize_row();
        }

        Ok(())
    }

    /// Prunes rows from data and offsets file and updates its configuration on disk
    pub fn prune_rows(&mut self, num_rows: usize) -> Result<(), NippyJarError> {
        // Each column of a row is one offset
        let num_offsets = num_rows * self.jar.columns;

        // Calculate the number of offsets to prune from in-memory list
        let offsets_prune_count = num_offsets.min(self.offsets.len().saturating_sub(1)); // last element is the expected size of the data file
        let remaining_to_prune = num_offsets.saturating_sub(offsets_prune_count);

        // Prune in-memory offsets if needed
        if offsets_prune_count > 0 {
            // Determine new length based on the offset to prune up to
            let new_len = self.offsets[(self.offsets.len() - 1) - offsets_prune_count]; // last element is the expected size of the data file
            self.offsets.truncate(self.offsets.len() - offsets_prune_count);

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
                // first byte is reserved for `bytes_per_offset`, which is 8 initially.
                let num_offsets = (length - 1) / bytes_per_offset;
                let new_num_offsets = num_offsets.saturating_sub(remaining_to_prune as u64);

                // If all rows are to be pruned
                if new_num_offsets <= 1 {
                    // <= 1 because the one offset would actually be the expected file data size
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

        self.offsets_file.seek(SeekFrom::End(0))?;
        self.data_file.seek(SeekFrom::End(0))?;

        self.jar.rows = self.jar.rows.saturating_sub(num_rows);
        if self.jar.rows == 0 {
            self.jar.max_row_size = 0;
        }
        self.jar.freeze_config()?;

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
        self.commit_offsets()?;

        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;

        Ok(())
    }

    /// Flushes offsets to disk.
    pub(crate) fn commit_offsets(&mut self) -> Result<(), NippyJarError> {
        // The last offset on disk can be the first offset on `self.offset_list` given how
        // `append_column()` works alongside commit. So we need to skip it.
        let mut last_offset_ondisk = None;

        if self.offsets_file.metadata()?.len() > 1 {
            self.offsets_file.seek(SeekFrom::End(-8))?;

            let mut buf = [0u8; 8];
            self.offsets_file.read_exact(&mut buf)?;
            last_offset_ondisk = Some(u64::from_le_bytes(buf));
        }

        self.offsets_file.seek(SeekFrom::End(0))?;

        // Appends new offsets to disk
        for offset in self.offsets.drain(..) {
            if let Some(last_offset_ondisk) = last_offset_ondisk.take() {
                if last_offset_ondisk == offset {
                    continue
                }
            }
            self.offsets_file.write_all(&offset.to_le_bytes())?;
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn column(&self) -> usize {
        self.column
    }

    #[cfg(test)]
    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }

    #[cfg(test)]
    pub fn offsets_mut(&mut self) -> &mut Vec<u64> {
        &mut self.offsets
    }
}
