use std::{
    fs::File,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

use eyre::OptionExt;
use reth_exex_types::ExExNotification;
use reth_tracing::tracing::debug;
use tracing::instrument;

/// The underlying WAL storage backed by a directory of files.
///
/// Each notification is represented by a single file that contains a MessagePack-encoded
/// notification.
#[derive(Debug, Clone)]
pub struct Storage {
    /// The path to the WAL file.
    path: PathBuf,
}

impl Storage {
    /// Creates a new instance of [`Storage`] backed by the file at the given path and creates
    /// it doesn't exist.
    pub(super) fn new(path: impl AsRef<Path>) -> eyre::Result<Self> {
        reth_fs_util::create_dir_all(&path)?;

        Ok(Self { path: path.as_ref().to_path_buf() })
    }

    fn file_path(&self, id: u32) -> PathBuf {
        self.path.join(format!("{id}.wal"))
    }

    fn parse_filename(filename: &str) -> eyre::Result<u32> {
        filename
            .strip_suffix(".wal")
            .and_then(|s| s.parse().ok())
            .ok_or_eyre(format!("failed to parse file name: {filename}"))
    }

    /// Removes notification for the given file ID from the storage.
    ///
    /// # Returns
    ///
    /// The size of the file that was removed in bytes, if any.
    #[instrument(skip(self))]
    fn remove_notification(&self, file_id: u32) -> Option<u64> {
        let path = self.file_path(file_id);
        let size = path.metadata().ok()?.len();

        match reth_fs_util::remove_file(self.file_path(file_id)) {
            Ok(()) => {
                debug!(target: "exex::wal::storage", "Notification was removed from the storage");
                Some(size)
            }
            Err(err) => {
                debug!(target: "exex::wal::storage", ?err, "Failed to remove notification from the storage");
                None
            }
        }
    }

    /// Returns the range of file IDs in the storage.
    ///
    /// If there are no files in the storage, returns `None`.
    pub(super) fn files_range(&self) -> eyre::Result<Option<RangeInclusive<u32>>> {
        let mut min_id = None;
        let mut max_id = None;

        for entry in reth_fs_util::read_dir(&self.path)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_id = Self::parse_filename(&file_name.to_string_lossy())?;

            min_id = min_id.map_or(Some(file_id), |min_id: u32| Some(min_id.min(file_id)));
            max_id = max_id.map_or(Some(file_id), |max_id: u32| Some(max_id.max(file_id)));
        }

        Ok(min_id.zip(max_id).map(|(min_id, max_id)| min_id..=max_id))
    }

    /// Removes notifications from the storage according to the given list of file IDs.
    ///
    /// # Returns
    ///
    /// Number of removed notifications and the total size of the removed files in bytes.
    pub(super) fn remove_notifications(
        &self,
        file_ids: impl IntoIterator<Item = u32>,
    ) -> eyre::Result<(usize, u64)> {
        let mut deleted_total = 0;
        let mut deleted_size = 0;

        for id in file_ids {
            if let Some(size) = self.remove_notification(id) {
                deleted_total += 1;
                deleted_size += size;
            }
        }

        Ok((deleted_total, deleted_size))
    }

    pub(super) fn iter_notifications(
        &self,
        range: RangeInclusive<u32>,
    ) -> impl Iterator<Item = eyre::Result<(u32, u64, ExExNotification)>> + '_ {
        range.map(move |id| {
            let (notification, size) =
                self.read_notification(id)?.ok_or_eyre("notification {id} not found")?;

            Ok((id, size, notification))
        })
    }

    /// Reads the notification from the file with the given ID.
    #[instrument(skip(self))]
    pub(super) fn read_notification(
        &self,
        file_id: u32,
    ) -> eyre::Result<Option<(ExExNotification, u64)>> {
        let file_path = self.file_path(file_id);
        debug!(target: "exex::wal::storage", ?file_path, "Reading notification from WAL");

        let mut file = match File::open(&file_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(reth_fs_util::FsPathError::open(err, &file_path).into()),
        };
        let size = file.metadata()?.len();

        // Deserialize using the bincode- and msgpack-compatible serde wrapper
        let notification: reth_exex_types::serde_bincode_compat::ExExNotification<'_> =
            rmp_serde::decode::from_read(&mut file).map_err(|err| {
                eyre::eyre!("failed to decode notification from {file_path:?}: {err:?}")
            })?;

        Ok(Some((notification.into(), size)))
    }

    /// Writes the notification to the file with the given ID.
    ///
    /// # Returns
    ///
    /// The size of the file that was written in bytes.
    #[instrument(skip(self, notification))]
    pub(super) fn write_notification(
        &self,
        file_id: u32,
        notification: &ExExNotification,
    ) -> eyre::Result<u64> {
        let file_path = self.file_path(file_id);
        debug!(target: "exex::wal::storage", ?file_path, "Writing notification to WAL");

        // Serialize using the bincode- and msgpack-compatible serde wrapper
        let notification =
            reth_exex_types::serde_bincode_compat::ExExNotification::from(notification);

        reth_fs_util::atomic_write_file(&file_path, |file| {
            rmp_serde::encode::write(file, &notification)
        })?;

        Ok(file_path.metadata()?.len())
    }
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

        let temp_dir = tempfile::tempdir()?;
        let storage = Storage::new(&temp_dir)?;

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
        let file_id = 0;
        storage.write_notification(file_id, &notification)?;
        let deserialized_notification = storage.read_notification(file_id)?;
        assert_eq!(
            deserialized_notification.map(|(notification, _)| notification),
            Some(notification)
        );

        Ok(())
    }
}
