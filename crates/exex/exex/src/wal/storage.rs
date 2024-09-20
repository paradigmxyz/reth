use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use reth_exex_types::ExExNotification;

#[derive(Debug)]
pub(super) struct Storage {
    /// The path to the WAL file.
    path: PathBuf,
    /// The file handle of the WAL file.
    file: File,
}

impl Storage {
    /// Creates a new instance of [`Storage`] backed by the file at the given path and creates
    /// it doesn't exist.
    pub(super) fn new(path: impl AsRef<Path>) -> eyre::Result<Self> {
        Ok(Self { path: path.as_ref().to_path_buf(), file: Self::open_file(&path)? })
    }

    /// Opens the file at the given path and creates it if it doesn't exist.
    pub(super) fn open_file(path: impl AsRef<Path>) -> std::io::Result<File> {
        File::options().read(true).write(true).create(true).truncate(false).open(path)
    }

    /// Returns the length of the underlying file in bytes.
    pub(super) fn bytes_len(&self) -> std::io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// Truncates the underlying file from the given byte offset (inclusive) to the end of the file.
    ///
    /// 1. Creates a new file and copies all notifications starting from the offset (inclusive) to
    ///    the end of the original file.
    /// 2. Renames the new file to the original file.
    ///
    /// # Returns
    ///
    /// The old and new underlying file sizes in bytes
    pub(super) fn truncate_from_offset(&mut self, offset: u64) -> eyre::Result<(u64, u64)> {
        // Seek the original file to the given offset
        self.file.seek(SeekFrom::Start(offset))?;

        // Open the temporary file for writing and copy the notifications with unfinalized blocks
        let tmp_file_path = self.path.with_extension("tmp");
        let new_file = File::create(&tmp_file_path)?;
        let mut file_reader = BufReader::new(&self.file);
        let mut new_file_writer = BufWriter::new(&new_file);
        loop {
            let buffer = file_reader.fill_buf()?;
            let buffer_len = buffer.len();
            if buffer_len == 0 {
                break
            }

            new_file_writer.write_all(buffer)?;
            file_reader.consume(buffer_len);
        }
        new_file_writer.flush()?;

        let old_size = self.file.metadata()?.len();
        let new_size = new_file.metadata()?.len();

        // Rename the temporary file to the WAL file and update the file handle with it
        reth_fs_util::rename(&tmp_file_path, &self.path)?;
        self.file = Self::open_file(&self.path)?;

        Ok((old_size, new_size))
    }

    /// Truncates the underlying file to the given byte offset (exclusive).
    pub(super) fn truncate_to_offset(&self, to_bytes_len: u64) -> eyre::Result<()> {
        self.file.set_len(to_bytes_len)?;
        Ok(())
    }

    /// Iterates over the notifications in the underlying file, decoding them and calling the
    /// provided closure with the length of the notification in bytes and the notification itself.
    /// Stops when the closure returns [`ControlFlow::Break`], or the end of the file is reached.
    pub(super) fn for_each_notification(
        &mut self,
        mut f: impl FnMut(usize, ExExNotification) -> eyre::Result<ControlFlow<()>>,
    ) -> eyre::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;

        let mut reader = BufReader::new(&self.file);
        loop {
            let Some((len, notification)) = read_notification(&mut reader)? else { break };
            f(len, notification)?;
        }
        Ok(())
    }

    /// Reads the notification from the underlying file at the given offset.
    pub(super) fn read_notification_at(
        &mut self,
        file_offset: u64,
    ) -> eyre::Result<Option<ExExNotification>> {
        self.file.seek(SeekFrom::Start(file_offset))?;
        Ok(read_notification(&mut self.file)?.map(|(_, notification)| notification))
    }

    /// Writes the notification to the end of the underlying file.
    pub(super) fn write_notification(
        &mut self,
        notification: &ExExNotification,
    ) -> eyre::Result<()> {
        write_notification(&mut self.file, notification)
    }
}

fn write_notification(w: &mut impl Write, notification: &ExExNotification) -> eyre::Result<()> {
    let data = rmp_serde::encode::to_vec(notification)?;
    w.write_all(&(data.len() as u32).to_le_bytes())?;
    w.write_all(&data)?;
    w.flush()?;
    Ok(())
}

fn read_notification(r: &mut impl Read) -> eyre::Result<Option<(usize, ExExNotification)>> {
    // Read the u32 length prefix
    let mut len_buf = [0; 4];
    let bytes_read = r.read(&mut len_buf)?;

    // EOF
    if bytes_read == 0 {
        return Ok(None)
    }

    // Convert the 4 bytes to a u32 to determine the length of the serialized notification
    let len = u32::from_le_bytes(len_buf) as usize;

    // Read the serialized notification
    let mut data = vec![0u8; len];
    r.read_exact(&mut data)?;

    // Deserialize the notification
    let notification: ExExNotification = rmp_serde::from_slice(&data)?;

    let total_len = len_buf.len() + data.len();
    Ok(Some((total_len, notification)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eyre::OptionExt;
    use reth_exex_types::ExExNotification;
    use reth_provider::Chain;
    use reth_testing_utils::generators::{self, random_block};

    use super::Storage;

    #[test]
    fn test_roundtrip() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let temp_file = tempfile::NamedTempFile::new()?;
        let mut storage = Storage::new(&temp_file)?;

        let old_block = random_block(&mut rng, 0, Default::default())
            .seal_with_senders()
            .ok_or_eyre("failed to recover senders")?;
        let new_block = random_block(&mut rng, 0, Default::default())
            .seal_with_senders()
            .ok_or_eyre("failed to recover senders")?;

        let notification = ExExNotification::ChainReorged {
            new: Arc::new(Chain::new(vec![new_block], Default::default(), None)),
            old: Arc::new(Chain::new(vec![old_block], Default::default(), None)),
        };

        // Do a round trip serialization and deserialization
        storage.write_notification(&notification)?;
        let deserialized_notification = storage.read_notification_at(0)?;
        assert_eq!(deserialized_notification, Some(notification));

        Ok(())
    }
}
