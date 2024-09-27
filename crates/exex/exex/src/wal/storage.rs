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

    fn file_path(&self, id: u64) -> PathBuf {
        self.path.join(format!("{id}.wal"))
    }

    fn parse_filename(filename: &str) -> eyre::Result<u64> {
        filename
            .strip_suffix(".wal")
            .and_then(|s| s.parse().ok())
            .ok_or_eyre(format!("failed to parse file name: {filename}"))
    }

    /// Removes notification for the given file ID from the storage.
    #[instrument(target = "exex::wal::storage", skip(self))]
    fn remove_notification(&self, file_id: u64) {
        match reth_fs_util::remove_file(self.file_path(file_id)) {
            Ok(()) => debug!("Notification was removed from the storage"),
            Err(err) => debug!(?err, "Failed to remove notification from the storage"),
        }
    }

    /// Returns the range of file IDs in the storage.
    ///
    /// If there are no files in the storage, returns `None`.
    pub(super) fn files_range(&self) -> eyre::Result<Option<RangeInclusive<u64>>> {
        let mut min_id = None;
        let mut max_id = None;

        for entry in reth_fs_util::read_dir(&self.path)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_id = Self::parse_filename(&file_name.to_string_lossy())?;

            min_id = min_id.map_or(Some(file_id), |min_id: u64| Some(min_id.min(file_id)));
            max_id = max_id.map_or(Some(file_id), |max_id: u64| Some(max_id.max(file_id)));
        }

        Ok(min_id.zip(max_id).map(|(min_id, max_id)| min_id..=max_id))
    }

    /// Removes notifications from the storage according to the given range.
    ///
    /// # Returns
    ///
    /// Number of removed notifications.
    pub(super) fn remove_notifications(&self, range: RangeInclusive<u64>) -> eyre::Result<usize> {
        for id in range.clone() {
            self.remove_notification(id);
        }

        Ok(range.count())
    }

    pub(super) fn iter_notifications(
        &self,
        range: RangeInclusive<u64>,
    ) -> impl Iterator<Item = eyre::Result<(u64, ExExNotification)>> + '_ {
        range.map(move |id| self.read_notification(id).map(|notification| (id, notification)))
    }

    /// Reads the notification from the file with the given id.
    #[instrument(target = "exex::wal::storage", skip(self))]
    pub(super) fn read_notification(&self, file_id: u64) -> eyre::Result<ExExNotification> {
        let file_path = self.file_path(file_id);
        debug!(?file_path, "Reading notification from WAL");

        let mut file = File::open(&file_path)?;
        // TODO(alexey): use rmp-serde when Alloy and Reth serde issues are resolved
        Ok(serde_json::from_reader(&mut file)?)
    }

    /// Writes the notification to the file with the given id.
    #[instrument(target = "exex::wal::storage", skip(self, notification))]
    pub(super) fn write_notification(
        &self,
        file_id: u64,
        notification: &ExExNotification,
    ) -> eyre::Result<()> {
        let file_path = self.file_path(file_id);
        debug!(?file_path, "Writing notification to WAL");

        Ok(reth_fs_util::atomic_write_file(&file_path, |file| {
            // TODO(alexey): use rmp-serde when Alloy and Reth serde issues are resolved
            serde_json::to_writer(file, notification)
        })?)
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
        assert_eq!(deserialized_notification, notification);

        Ok(())
    }
}
