use crate::{compression::Compression, ColumnResult, NippyJar, NippyJarError, NippyJarHeader};
use std::{
    cmp::Ordering,
    fs::{File, OpenOptions},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Size of one offset in bytes.
const OFFSET_SIZE_BYTES: u64 = 8;

/// Writer of [`NippyJar`]. Handles table data and offsets only.
///
/// Table data is written directly to disk, while offsets and configuration need to be flushed by
/// calling `commit()`.
///
/// ## Offset file layout
/// The first byte is the size of a single offset in bytes, `m`.
/// Then, the file contains `n` entries, each with a size of `m`. Each entry represents an offset,
/// except for the last entry, which represents both the total size of the data file, as well as the
/// next offset to write new data to.
///
/// ## Data file layout
/// The data file is represented just as a sequence of bytes of data without any delimiters
#[derive(Debug)]
pub struct NippyJarWriter<H: NippyJarHeader = ()> {
    /// Associated [`NippyJar`], containing all necessary configurations for data
    /// handling.
    jar: NippyJar<H>,
    /// File handle to where the data is stored.
    data_file: BufWriter<File>,
    /// File handle to where the offsets are stored.
    offsets_file: BufWriter<File>,
    /// Temporary buffer to reuse when compressing data.
    tmp_buf: Vec<u8>,
    /// Used to find the maximum uncompressed size of a row in a jar.
    uncompressed_row_size: usize,
    /// Partial offset list which hasn't been flushed to disk.
    offsets: Vec<u64>,
    /// Column where writer is going to write next.
    column: usize,
}

impl<H: NippyJarHeader> NippyJarWriter<H> {
    /// Creates a [`NippyJarWriter`] from [`NippyJar`].
    pub fn new(mut jar: NippyJar<H>) -> Result<Self, NippyJarError> {
        let (data_file, offsets_file, is_created) =
            Self::create_or_open_files(jar.data_path(), &jar.offsets_path())?;

        // Makes sure we don't have dangling data and offset files
        jar.freeze_config()?;

        let mut writer = Self {
            jar,
            data_file: BufWriter::new(data_file),
            offsets_file: BufWriter::new(offsets_file),
            tmp_buf: Vec::with_capacity(1_000_000),
            uncompressed_row_size: 0,
            offsets: Vec::with_capacity(1_000_000),
            column: 0,
        };

        // If we are opening a previously created jar, we need to check its consistency, and make
        // changes if necessary.
        if !is_created {
            writer.check_consistency_and_heal()?;
            writer.commit()?;
        }

        Ok(writer)
    }

    /// Returns a reference to `H` of [`NippyJar`]
    pub fn user_header(&self) -> &H {
        &self.jar.user_header
    }

    /// Returns a mutable reference to `H` of [`NippyJar`]
    pub fn user_header_mut(&mut self) -> &mut H {
        &mut self.jar.user_header
    }

    /// Gets total writer rows in jar.
    pub fn rows(&self) -> usize {
        self.jar.rows()
    }

    /// Consumes the writer and returns the associated [`NippyJar`].
    pub fn into_jar(self) -> NippyJar<H> {
        self.jar
    }

    fn create_or_open_files(
        data: &Path,
        offsets: &Path,
    ) -> Result<(File, File, bool), NippyJarError> {
        let is_created = !data.exists() || !offsets.exists();

        if !data.exists() {
            // File::create is write-only (no reading possible)
            File::create(data)?;
        }

        let mut data_file = OpenOptions::new().read(true).write(true).open(data)?;
        data_file.seek(SeekFrom::End(0))?;

        if !offsets.exists() {
            // File::create is write-only (no reading possible)
            File::create(offsets)?;
        }

        let mut offsets_file = OpenOptions::new().read(true).write(true).open(offsets)?;

        // First byte of the offset file is the size of one offset in bytes
        offsets_file.write_all(&[OFFSET_SIZE_BYTES as u8])?;
        offsets_file.seek(SeekFrom::End(0))?;

        Ok((data_file, offsets_file, is_created))
    }

    /// Performs consistency checks on the [`NippyJar`] file and acts upon any issues:
    /// * Is the offsets file size expected?
    /// * Is the data file size expected?
    ///
    /// This is based on the assumption that [`NippyJar`] configuration is **always** the last one
    /// to be updated when something is written, as by the `commit()` function shows.
    fn check_consistency_and_heal(&mut self) -> Result<(), NippyJarError> {
        let reader = self.jar.open_data_reader()?;

        // When an offset size is smaller than the initial (8), we are dealing with immutable
        // data.
        if reader.offset_size() != OFFSET_SIZE_BYTES {
            return Err(NippyJarError::FrozenJar)
        }

        let expected_offsets_file_size = 1 + // first byte is the size of one offset
            OFFSET_SIZE_BYTES * self.jar.rows as u64 * self.jar.columns as u64 + // `offset size * num rows * num columns`
            OFFSET_SIZE_BYTES; // expected size of the data file
        let actual_offsets_file_size = self.offsets_file.get_ref().metadata()?.len();

        // Offsets configuration wasn't properly committed
        match expected_offsets_file_size.cmp(&actual_offsets_file_size) {
            Ordering::Less => {
                // Happened during an appending job
                // TODO: ideally we could truncate until the last offset of the last column of the
                //  last row inserted
                self.offsets_file.get_mut().set_len(expected_offsets_file_size)?;
            }
            Ordering::Greater => {
                // Happened during a pruning job
                // `num rows = (file size - 1 - size of one offset) / num columns`
                self.jar.rows = ((actual_offsets_file_size.
                    saturating_sub(1). // first byte is the size of one offset
                    saturating_sub(OFFSET_SIZE_BYTES) / // expected size of the data file
                    (self.jar.columns as u64)) /
                    OFFSET_SIZE_BYTES) as usize;

                // Freeze row count changed
                self.jar.freeze_config()?;
            }
            Ordering::Equal => {}
        }

        // last offset should match the data_file_len
        let last_offset = reader.reverse_offset(0)?;
        let data_file_len = self.data_file.get_ref().metadata()?.len();

        // Offset list wasn't properly committed
        match last_offset.cmp(&data_file_len) {
            Ordering::Less => {
                // Happened during an appending job, so we need to truncate the data, since there's
                // no way to recover it.
                self.data_file.get_mut().set_len(last_offset)?;
            }
            Ordering::Greater => {
                // Happened during a pruning job, so we need to reverse iterate offsets until we
                // find the matching one.
                for index in 0..reader.offsets_count()? {
                    let offset = reader.reverse_offset(index + 1)?;
                    if offset == data_file_len {
                        let new_len = self
                            .offsets_file
                            .get_ref()
                            .metadata()?
                            .len()
                            .saturating_sub(OFFSET_SIZE_BYTES * (index as u64 + 1));
                        self.offsets_file.get_mut().set_len(new_len)?;

                        drop(reader);

                        // Since we decrease the offset list, we need to check the consistency of
                        // `self.jar.rows` again
                        self.check_consistency_and_heal()?;
                        break
                    }
                }
            }
            Ordering::Equal => {}
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

                let written = self.write_column(value.as_ref())?;

                // Last offset represents the size of the data file if no more data is to be
                // appended. Otherwise, represents the offset of the next data item.
                self.offsets.push(self.offsets.last().expect("qed") + written as u64);
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
    fn write_column(&mut self, value: &[u8]) -> Result<usize, NippyJarError> {
        self.uncompressed_row_size += value.len();
        let len = if let Some(compression) = &self.jar.compressor {
            let before = self.tmp_buf.len();
            let len = compression.compress_to(value, &mut self.tmp_buf)?;
            self.data_file.write_all(&self.tmp_buf[before..before + len])?;
            len
        } else {
            self.data_file.write_all(value)?;
            value.len()
        };

        self.column += 1;

        if self.jar.columns == self.column {
            self.finalize_row();
        }

        Ok(len)
    }

    /// Prunes rows from data and offsets file and updates its configuration on disk
    pub fn prune_rows(&mut self, num_rows: usize) -> Result<(), NippyJarError> {
        self.offsets_file.flush()?;
        self.data_file.flush()?;

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
            self.data_file.get_mut().set_len(new_len)?;
        }

        // Prune from on-disk offset list if there are still rows left to prune
        if remaining_to_prune > 0 {
            // Get the current length of the on-disk offset file
            let length = self.offsets_file.get_ref().metadata()?.len();

            // Handle non-empty offset file
            if length > 1 {
                // first byte is reserved for `bytes_per_offset`, which is 8 initially.
                let num_offsets = (length - 1) / OFFSET_SIZE_BYTES;

                if remaining_to_prune as u64 > num_offsets {
                    return Err(NippyJarError::InvalidPruning(
                        num_offsets,
                        remaining_to_prune as u64,
                    ))
                }

                let new_num_offsets = num_offsets.saturating_sub(remaining_to_prune as u64);

                // If all rows are to be pruned
                if new_num_offsets <= 1 {
                    // <= 1 because the one offset would actually be the expected file data size
                    self.offsets_file.get_mut().set_len(1)?;
                    self.data_file.get_mut().set_len(0)?;
                } else {
                    // Calculate the new length for the on-disk offset list
                    let new_len = 1 + new_num_offsets * OFFSET_SIZE_BYTES;
                    // Seek to the position of the last offset
                    self.offsets_file
                        .seek(SeekFrom::Start(new_len.saturating_sub(OFFSET_SIZE_BYTES)))?;
                    // Read the last offset value
                    let mut last_offset = [0u8; OFFSET_SIZE_BYTES as usize];
                    self.offsets_file.get_ref().read_exact(&mut last_offset)?;
                    let last_offset = u64::from_le_bytes(last_offset);

                    // Update the lengths of both the offsets and data files
                    self.offsets_file.get_mut().set_len(new_len)?;
                    self.data_file.get_mut().set_len(last_offset)?;
                }
            } else {
                return Err(NippyJarError::InvalidPruning(0, remaining_to_prune as u64))
            }
        }

        self.offsets_file.get_ref().sync_all()?;
        self.data_file.get_ref().sync_all()?;

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
        self.data_file.flush()?;
        self.data_file.get_ref().sync_all()?;

        self.commit_offsets()?;

        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;

        Ok(())
    }

    #[cfg(feature = "test-utils")]
    pub fn commit_without_sync_all(&mut self) -> Result<(), NippyJarError> {
        self.data_file.flush()?;

        self.commit_offsets_without_sync_all()?;

        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;

        Ok(())
    }

    /// Flushes offsets to disk.
    pub(crate) fn commit_offsets(&mut self) -> Result<(), NippyJarError> {
        self.commit_offsets_inner()?;
        self.offsets_file.get_ref().sync_all()?;

        Ok(())
    }

    #[cfg(feature = "test-utils")]
    fn commit_offsets_without_sync_all(&mut self) -> Result<(), NippyJarError> {
        self.commit_offsets_inner()
    }

    /// Flushes offsets to disk.
    ///
    /// CAUTION: Does not call `sync_all` on the offsets file and requires a manual call to
    /// `self.offsets_file.get_ref().sync_all()`.
    fn commit_offsets_inner(&mut self) -> Result<(), NippyJarError> {
        // The last offset on disk can be the first offset of `self.offsets` given how
        // `append_column()` works alongside commit. So we need to skip it.
        let mut last_offset_ondisk = if self.offsets_file.get_ref().metadata()?.len() > 1 {
            self.offsets_file.seek(SeekFrom::End(-(OFFSET_SIZE_BYTES as i64)))?;
            let mut buf = [0u8; OFFSET_SIZE_BYTES as usize];
            self.offsets_file.get_ref().read_exact(&mut buf)?;
            Some(u64::from_le_bytes(buf))
        } else {
            None
        };

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
        self.offsets_file.flush()?;

        Ok(())
    }

    #[cfg(test)]
    pub fn max_row_size(&self) -> usize {
        self.jar.max_row_size
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

    #[cfg(test)]
    pub fn offsets_path(&self) -> std::path::PathBuf {
        self.jar.offsets_path()
    }

    #[cfg(test)]
    pub fn data_path(&self) -> &Path {
        self.jar.data_path()
    }
}
