use crate::{
    compression::Compression, ColumnResult, NippyJar, NippyJarChecker, NippyJarError,
    NippyJarHeader,
};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Size of one offset in bytes.
pub(crate) const OFFSET_SIZE_BYTES: u8 = 8;

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
    /// Whether the writer has changed data that needs to be committed.
    dirty: bool,
    pending_truncation: Option<PendingTruncation>,
}

#[derive(Debug, Clone, Copy)]
struct PendingTruncation {
    offsets_len: u64,
    data_len: u64,
}

impl<H: NippyJarHeader> NippyJarWriter<H> {
    /// Creates a [`NippyJarWriter`] from [`NippyJar`].
    ///
    /// Uses the persisted configuration when present and attempts to heal inconsistent files.
    /// Missing files are recreated only for empty jars with an initial offsets prefix.
    pub fn new(mut jar: NippyJar<H>) -> Result<Self, NippyJarError> {
        let config_exists = jar.config_path().try_exists()?;
        // Disk configuration is authoritative, including zero-row user headers.
        if config_exists {
            jar = NippyJar::load(jar.data_path())?;
        }
        let data_exists = jar.data_path().try_exists()?;
        let offsets_exist = jar.offsets_path().try_exists()?;
        if !config_exists &&
            (offsets_exist ||
                (data_exists && reth_fs_util::metadata(jar.data_path())?.len() != 0))
        {
            return Err(NippyJarError::MissingFile(jar.config_path()))
        }
        let is_created = !data_exists ||
            !offsets_exist ||
            (jar.rows() == 0 &&
                reth_fs_util::metadata(jar.offsets_path())?.len() <
                    1 + u64::from(OFFSET_SIZE_BYTES));

        if is_created {
            if jar.rows() != 0 ||
                (data_exists && reth_fs_util::metadata(jar.data_path())?.len() != 0)
            {
                return Err(NippyJarError::InconsistentState)
            }
            if offsets_exist {
                if reth_fs_util::metadata(jar.offsets_path())?.len() >
                    1 + u64::from(OFFSET_SIZE_BYTES)
                {
                    return Err(NippyJarError::InconsistentState)
                }
                let offsets = reth_fs_util::read(jar.offsets_path())?;
                let mut initial_offset = [0; 1 + OFFSET_SIZE_BYTES as usize];
                initial_offset[0] = OFFSET_SIZE_BYTES;
                if !initial_offset.starts_with(&offsets) {
                    return Err(NippyJarError::InconsistentState)
                }
            }
        }

        let (data_file, offsets_file) =
            Self::create_or_open_files(jar.data_path(), &jar.offsets_path())?;

        let (jar, data_file, offsets_file) = if is_created {
            // Rewrite an interrupted initial offset, including a partially written one.
            let mut offsets_file = offsets_file;
            offsets_file.set_len(0)?;
            offsets_file.seek(SeekFrom::Start(0))?;
            offsets_file.write_all(&[OFFSET_SIZE_BYTES])?;
            offsets_file.write_all(&[0; OFFSET_SIZE_BYTES as usize])?;
            // Preserve the existing initialization order: data and offsets before configuration.
            jar.freeze_config()?;
            (jar, BufWriter::new(data_file), BufWriter::new(offsets_file))
        } else {
            // If we are opening a previously created jar, we need to check its consistency, and
            // make changes if necessary.
            let mut checker = NippyJarChecker::new(jar);
            checker.ensure_consistency()?;

            let NippyJarChecker { jar, data_file, offsets_file } = checker;

            // Calling ensure_consistency, will fill data_file and offsets_file
            (jar, data_file.expect("qed"), offsets_file.expect("qed"))
        };

        let mut writer = Self {
            jar,
            data_file,
            offsets_file,
            tmp_buf: Vec::with_capacity(1_000_000),
            uncompressed_row_size: 0,
            offsets: Vec::with_capacity(1_000_000),
            column: 0,
            dirty: false,
            pending_truncation: None,
        };

        if !is_created {
            // Commit any potential heals done above.
            writer.commit()?;
        }

        Ok(writer)
    }

    /// Returns a reference to `H` of [`NippyJar`]
    pub const fn user_header(&self) -> &H {
        &self.jar.user_header
    }

    /// Returns a mutable reference to `H` of [`NippyJar`].
    ///
    /// Since there's no way of knowing if `H` has been actually changed, this sets `self.dirty` to
    /// true.
    pub const fn user_header_mut(&mut self) -> &mut H {
        self.dirty = true;
        &mut self.jar.user_header
    }

    /// Returns whether there are changes that need to be committed.
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Sets writer as dirty.
    pub const fn set_dirty(&mut self) {
        self.dirty = true
    }

    /// Gets total writer rows in jar.
    pub const fn rows(&self) -> usize {
        self.jar.rows()
    }

    /// Consumes the writer and returns the associated [`NippyJar`].
    pub fn into_jar(self) -> NippyJar<H> {
        self.jar
    }

    fn create_or_open_files(data: &Path, offsets: &Path) -> Result<(File, File), NippyJarError> {
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

        let offsets_file = OpenOptions::new().read(true).write(true).open(offsets)?;
        Ok((data_file, offsets_file))
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
        self.apply_pending_truncation()?;
        self.dirty = true;

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
        self.prune_rows_deferred(num_rows)?;
        self.apply_pending_truncation()?;
        Ok(())
    }

    /// Prunes rows like [`Self::prune_rows`], but leaves the committed rows' bytes on disk until
    /// [`Self::apply_pending_truncation`].
    pub fn prune_rows_deferred(&mut self, num_rows: usize) -> Result<(), NippyJarError> {
        self.apply_pending_truncation()?;
        self.dirty = true;

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
                let num_offsets = (length - 1) / OFFSET_SIZE_BYTES as u64;

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
                    //
                    // When no rows remain, keep the offset size byte and the final offset (data
                    // file size = 0). This maintains the same structure as when
                    // a file is initially created.
                    // See `NippyJarWriter::new` for the initial file format.
                    self.pending_truncation = Some(PendingTruncation {
                        offsets_len: 1 + OFFSET_SIZE_BYTES as u64,
                        data_len: 0,
                    });
                } else {
                    // Calculate the new length for the on-disk offset list
                    let new_len = 1 + new_num_offsets * OFFSET_SIZE_BYTES as u64;
                    // Seek to the position of the last offset
                    self.offsets_file
                        .seek(SeekFrom::Start(new_len.saturating_sub(OFFSET_SIZE_BYTES as u64)))?;
                    // Read the last offset value
                    let mut last_offset = [0u8; OFFSET_SIZE_BYTES as usize];
                    self.offsets_file.get_ref().read_exact(&mut last_offset)?;
                    let last_offset = u64::from_le_bytes(last_offset);

                    // Update the lengths of both the offsets and data files
                    self.pending_truncation =
                        Some(PendingTruncation { offsets_len: new_len, data_len: last_offset });
                }
            } else {
                return Err(NippyJarError::InvalidPruning(0, remaining_to_prune as u64))
            }
        }

        if remaining_to_prune > 0 {
            // Every uncommitted row was pruned, so the remaining in-memory offset (the previous
            // data file end) is stale: the on-disk prune shrank the data file below it.
            self.offsets.clear();
        } else {
            // The config written below counts the uncommitted rows that were not pruned, so their
            // offsets must be on disk too. If we crash before the config is written, the offsets
            // file is longer than the config expects and the checker heals it as an interrupted
            // append.
            self.commit_offsets_inner()?;
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

    /// Truncates the files to the rows kept by [`Self::prune_rows_deferred`].
    ///
    /// Returns `false` if no truncation was pending.
    pub fn apply_pending_truncation(&mut self) -> Result<bool, NippyJarError> {
        let Some(PendingTruncation { offsets_len, data_len }) = self.pending_truncation else {
            return Ok(false)
        };

        self.offsets_file.get_mut().set_len(offsets_len)?;
        self.data_file.get_mut().set_len(data_len)?;

        self.offsets_file.get_ref().sync_all()?;
        self.data_file.get_ref().sync_all()?;

        self.offsets_file.seek(SeekFrom::End(0))?;
        self.data_file.seek(SeekFrom::End(0))?;
        self.pending_truncation = None;

        Ok(true)
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
        self.sync_all()?;
        self.finalize()?;
        Ok(())
    }

    /// Syncs data and offsets to disk.
    ///
    /// This does NOT commit the configuration. Call [`Self::finalize`] after to write the
    /// configuration and mark the writer as clean.
    pub fn sync_all(&mut self) -> Result<(), NippyJarError> {
        self.data_file.flush()?;
        self.data_file.get_ref().sync_all()?;

        self.commit_offsets()?;
        Ok(())
    }

    /// Commits configuration to disk and marks the writer as clean.
    ///
    /// Must be called after [`Self::sync_all`] to complete the commit.
    pub fn finalize(&mut self) -> Result<(), NippyJarError> {
        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;
        self.dirty = false;

        Ok(())
    }

    /// Commits changes to the data file and offsets without synchronizing all data to disk.
    ///
    /// This function flushes the buffered data to the data file and commits the offsets,
    /// but it does not guarantee that all data is synchronized to persistent storage.
    #[cfg(feature = "test-utils")]
    pub fn commit_without_sync_all(&mut self) -> Result<(), NippyJarError> {
        self.data_file.flush()?;

        self.commit_offsets_without_sync_all()?;

        // Flushes `max_row_size` and total `rows` to disk.
        self.jar.freeze_config()?;
        self.dirty = false;

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
            if let Some(last_offset_ondisk) = last_offset_ondisk.take() &&
                last_offset_ondisk == offset
            {
                continue
            }
            self.offsets_file.write_all(&offset.to_le_bytes())?;
        }
        self.offsets_file.flush()?;

        Ok(())
    }

    /// Returns the maximum row size for the associated [`NippyJar`].
    #[cfg(test)]
    pub const fn max_row_size(&self) -> usize {
        self.jar.max_row_size
    }

    /// Returns the column index of the current checker instance.
    #[cfg(test)]
    pub const fn column(&self) -> usize {
        self.column
    }

    /// Returns a reference to the offsets vector.
    #[cfg(test)]
    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }

    /// Returns a mutable reference to the offsets vector.
    #[cfg(test)]
    pub const fn offsets_mut(&mut self) -> &mut Vec<u64> {
        &mut self.offsets
    }

    /// Returns the path to the offsets file for the associated [`NippyJar`].
    #[cfg(test)]
    pub fn offsets_path(&self) -> std::path::PathBuf {
        self.jar.offsets_path()
    }

    /// Returns the path to the data file for the associated [`NippyJar`].
    #[cfg(test)]
    pub fn data_path(&self) -> &Path {
        self.jar.data_path()
    }

    /// Returns a mutable reference to the buffered writer for the data file.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn data_file(&mut self) -> &mut BufWriter<File> {
        &mut self.data_file
    }

    /// Returns a reference to the associated [`NippyJar`] instance.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn jar(&self) -> &NippyJar<H> {
        &self.jar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resumes_empty_bare_data_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jar");
        fs::write(&path, []).unwrap();
        let writer = NippyJarWriter::new(NippyJar::new(1, &path, 42u64)).unwrap();
        assert_eq!(writer.rows(), 0);
        assert_eq!(*writer.user_header(), 42);
        drop(writer);
        let jar = NippyJar::<u64>::load(&path).unwrap();
        NippyJarChecker::new(jar).check_consistency().unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
    }

    #[test]
    fn resumes_every_initialization_prefix() {
        // Recover empty jars with persisted configuration and missing or partial initial files.
        for data_exists in [false, true] {
            for offsets_len in 0..=2 + usize::from(OFFSET_SIZE_BYTES) {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("jar");
                let jar = NippyJar::new(1, &path, 42u64);
                jar.freeze_config().unwrap();
                if data_exists {
                    fs::write(&path, []).unwrap();
                }
                if offsets_len > 0 {
                    let mut offsets = vec![0; offsets_len - 1];
                    if !offsets.is_empty() {
                        offsets[0] = OFFSET_SIZE_BYTES;
                    }
                    fs::write(jar.offsets_path(), offsets).unwrap();
                }
                let mut writer = NippyJarWriter::new(NippyJar::new(1, &path, 99u64)).unwrap();
                assert_eq!(*writer.user_header(), 42);
                writer
                    .append_column(Some(Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                        b"payload".as_slice(),
                    )))
                    .unwrap();
                writer.commit().unwrap();
                drop(writer);
                let jar = NippyJar::<u64>::load(&path).unwrap();
                assert_eq!(jar.rows(), 1);
                assert_eq!(*jar.user_header(), 42);
                NippyJarChecker::new(jar).check_consistency().unwrap();
                assert_eq!(fs::read(&path).unwrap(), b"payload");
            }
        }
    }

    #[test]
    fn incomplete_nonempty_jar_is_preserved() {
        for missing_data in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jar");
            let mut writer = NippyJarWriter::new(NippyJar::new_without_header(1, &path)).unwrap();
            writer
                .append_column(Some(Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    b"payload".as_slice(),
                )))
                .unwrap();
            writer.commit().unwrap();
            drop(writer);
            let missing = if missing_data { path.clone() } else { path.with_extension("off") };
            fs::remove_file(&missing).unwrap();
            let config = fs::read(path.with_extension("conf")).unwrap();
            let jar = NippyJar::load_without_header(&path).unwrap();
            assert!(matches!(NippyJarWriter::new(jar), Err(NippyJarError::InconsistentState)));
            assert_eq!(fs::read(path.with_extension("conf")).unwrap(), config);
            assert!(!missing.exists());
            if !missing_data {
                assert_eq!(fs::read(&path).unwrap(), b"payload");
            }
        }
    }

    #[test]
    fn missing_config_does_not_reset_existing_files() {
        for (data, offsets) in [
            (Some(b"payload".as_slice()), None),
            (None, Some([8; 9])),
            (Some(b"".as_slice()), Some([8; 9])),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jar");
            if let Some(data) = data {
                fs::write(&path, data).unwrap();
            }
            if let Some(offsets) = offsets {
                fs::write(path.with_extension("off"), offsets).unwrap();
            }
            assert!(matches!(
                NippyJarWriter::new(NippyJar::new_without_header(1, &path)),
                Err(NippyJarError::MissingFile(missing)) if missing == path.with_extension("conf")
            ));
            assert!(!path.with_extension("conf").exists());
            if let Some(data) = data {
                assert_eq!(fs::read(&path).unwrap(), data);
            } else {
                assert!(!path.exists());
            }
            if let Some(offsets) = offsets {
                assert_eq!(fs::read(path.with_extension("off")).unwrap(), offsets);
            } else {
                assert!(!path.with_extension("off").exists());
            }
        }
    }

    #[test]
    fn reopening_uses_committed_rows_and_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jar");
        let mut writer = NippyJarWriter::new(NippyJar::new(1, &path, 42u64)).unwrap();
        writer
            .append_column(Some(Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                b"payload".as_slice(),
            )))
            .unwrap();
        writer.commit().unwrap();
        drop(writer);
        let config = fs::read(path.with_extension("conf")).unwrap();
        let writer = NippyJarWriter::new(NippyJar::new(1, &path, 99u64)).unwrap();
        assert_eq!(writer.rows(), 1);
        assert_eq!(*writer.user_header(), 42);
        assert_eq!(fs::read(&path).unwrap(), b"payload");
        assert_eq!(fs::read(path.with_extension("conf")).unwrap(), config);
    }

    #[test]
    fn invalid_initial_offset_is_preserved() {
        for offsets in [vec![0], vec![OFFSET_SIZE_BYTES, 1], vec![0; 10]] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("jar");
            let jar = NippyJar::new_without_header(1, &path);
            jar.freeze_config().unwrap();
            let config = fs::read(jar.config_path()).unwrap();
            fs::write(path.with_extension("off"), &offsets).unwrap();
            assert!(matches!(
                NippyJarWriter::new(NippyJar::new_without_header(1, &path)),
                Err(NippyJarError::InconsistentState)
            ));
            assert_eq!(fs::read(path.with_extension("off")).unwrap(), offsets);
            assert!(!path.exists());
            assert_eq!(fs::read(path.with_extension("conf")).unwrap(), config);
        }
    }
}
