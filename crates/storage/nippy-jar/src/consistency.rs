use crate::{writer::OFFSET_SIZE_BYTES, NippyJar, NippyJarError, NippyJarHeader};
use std::{
    cmp::Ordering,
    fs::{File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom},
    path::Path,
};

/// Performs consistency checks or heals on the [`NippyJar`] file
/// * Is the offsets file size expected?
/// * Is the data file size expected?
///
/// This is based on the assumption that [`NippyJar`] configuration is **always** the last one
/// to be updated when something is written, as by the `NippyJarWriter::commit()` function shows.
///
/// **For checks (read-only) use `check_consistency` method.**
///
/// **For heals (read-write) use `ensure_consistency` method.**
#[derive(Debug)]
pub struct NippyJarChecker<H: NippyJarHeader = ()> {
    /// Associated [`NippyJar`], containing all necessary configurations for data
    /// handling.
    pub(crate) jar: NippyJar<H>,
    /// File handle to where the data is stored.
    pub(crate) data_file: Option<BufWriter<File>>,
    /// File handle to where the offsets are stored.
    pub(crate) offsets_file: Option<BufWriter<File>>,
}

impl<H: NippyJarHeader> NippyJarChecker<H> {
    pub const fn new(jar: NippyJar<H>) -> Self {
        Self { jar, data_file: None, offsets_file: None }
    }

    /// It will throw an error if the [`NippyJar`] is in a inconsistent state.
    pub fn check_consistency(&mut self) -> Result<(), NippyJarError> {
        self.handle_consistency(ConsistencyFailStrategy::ThrowError)
    }

    /// It will attempt to heal if the [`NippyJar`] is in a inconsistent state.
    ///
    /// **ATTENTION**: disk commit should be handled externally by consuming `Self`
    pub fn ensure_consistency(&mut self) -> Result<(), NippyJarError> {
        self.handle_consistency(ConsistencyFailStrategy::Heal)
    }

    fn handle_consistency(&mut self, mode: ConsistencyFailStrategy) -> Result<(), NippyJarError> {
        self.load_files(mode)?;
        let mut reader = self.jar.open_data_reader()?;

        // When an offset size is smaller than the initial (8), we are dealing with immutable
        // data.
        if reader.offset_size() != OFFSET_SIZE_BYTES {
            return Err(NippyJarError::FrozenJar)
        }

        let expected_offsets_file_size: u64 = (1 + // first byte is the size of one offset
                OFFSET_SIZE_BYTES as usize* self.jar.rows * self.jar.columns + // `offset size * num rows * num columns`
                OFFSET_SIZE_BYTES as usize) as u64; // expected size of the data file
        let actual_offsets_file_size = self.offsets_file().get_ref().metadata()?.len();

        if mode.should_err() &&
            expected_offsets_file_size.cmp(&actual_offsets_file_size) != Ordering::Equal
        {
            return Err(NippyJarError::InconsistentState)
        }

        // Offsets configuration wasn't properly committed
        match expected_offsets_file_size.cmp(&actual_offsets_file_size) {
            Ordering::Less => {
                // Happened during an appending job
                // TODO: ideally we could truncate until the last offset of the last column of the
                //  last row inserted

                // Windows has locked the file with the mmap handle, so we need to drop it
                drop(reader);

                self.offsets_file().get_mut().set_len(expected_offsets_file_size)?;
                reader = self.jar.open_data_reader()?;
            }
            Ordering::Greater => {
                // Happened during a pruning job
                // `num rows = (file size - 1 - size of one offset) / num columns`
                self.jar.rows = ((actual_offsets_file_size.
                        saturating_sub(1). // first byte is the size of one offset
                        saturating_sub(OFFSET_SIZE_BYTES as u64) / // expected size of the data file
                        (self.jar.columns as u64)) /
                    OFFSET_SIZE_BYTES as u64) as usize;

                // Freeze row count changed
                self.jar.freeze_config()?;
            }
            Ordering::Equal => {}
        }

        // last offset should match the data_file_len
        let last_offset = reader.reverse_offset(0)?;
        let data_file_len = self.data_file().get_ref().metadata()?.len();

        if mode.should_err() && last_offset.cmp(&data_file_len) != Ordering::Equal {
            return Err(NippyJarError::InconsistentState)
        }

        // Offset list wasn't properly committed
        match last_offset.cmp(&data_file_len) {
            Ordering::Less => {
                // Windows has locked the file with the mmap handle, so we need to drop it
                drop(reader);

                // Happened during an appending job, so we need to truncate the data, since there's
                // no way to recover it.
                self.data_file().get_mut().set_len(last_offset)?;
            }
            Ordering::Greater => {
                // Happened during a pruning job, so we need to reverse iterate offsets until we
                // find the matching one.
                for index in 0..reader.offsets_count()? {
                    let offset = reader.reverse_offset(index + 1)?;
                    // It would only be equal if the previous row was fully pruned.
                    if offset <= data_file_len {
                        let new_len = self
                            .offsets_file()
                            .get_ref()
                            .metadata()?
                            .len()
                            .saturating_sub(OFFSET_SIZE_BYTES as u64 * (index as u64 + 1));

                        // Windows has locked the file with the mmap handle, so we need to drop it
                        drop(reader);

                        self.offsets_file().get_mut().set_len(new_len)?;

                        // Since we decrease the offset list, we need to check the consistency of
                        // `self.jar.rows` again
                        self.handle_consistency(ConsistencyFailStrategy::Heal)?;
                        break
                    }
                }
            }
            Ordering::Equal => {}
        }

        self.offsets_file().seek(SeekFrom::End(0))?;
        self.data_file().seek(SeekFrom::End(0))?;

        Ok(())
    }

    /// Loads data and offsets files.
    fn load_files(&mut self, mode: ConsistencyFailStrategy) -> Result<(), NippyJarError> {
        let load_file = |path: &Path| -> Result<BufWriter<File>, NippyJarError> {
            let path = path
                .exists()
                .then_some(path)
                .ok_or_else(|| NippyJarError::MissingFile(path.to_path_buf()))?;
            Ok(BufWriter::new(OpenOptions::new().read(true).write(mode.should_heal()).open(path)?))
        };
        self.data_file = Some(load_file(self.jar.data_path())?);
        self.offsets_file = Some(load_file(&self.jar.offsets_path())?);
        Ok(())
    }

    /// Returns a mutable reference to offsets file.
    ///
    /// **Panics** if it does not exist.
    fn offsets_file(&mut self) -> &mut BufWriter<File> {
        self.offsets_file.as_mut().expect("should exist")
    }

    /// Returns a mutable reference to data file.
    ///
    /// **Panics** if it does not exist.
    fn data_file(&mut self) -> &mut BufWriter<File> {
        self.data_file.as_mut().expect("should exist")
    }
}

/// Strategy on encountering an inconsistent state on [`NippyJarChecker`].
#[derive(Debug, Copy, Clone)]
enum ConsistencyFailStrategy {
    /// Writer should heal.
    Heal,
    /// Writer should throw an error.
    ThrowError,
}

impl ConsistencyFailStrategy {
    /// Whether writer should heal.
    const fn should_heal(&self) -> bool {
        matches!(self, Self::Heal)
    }

    /// Whether writer should throw an error.
    const fn should_err(&self) -> bool {
        matches!(self, Self::ThrowError)
    }
}
