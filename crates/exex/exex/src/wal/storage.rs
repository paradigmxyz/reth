use std::{
    fs::File,
    io::{Read, Write},
    ops::{Bound, RangeBounds, RangeInclusive},
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
#[derive(Debug)]
pub(super) struct Storage {
    /// The path to the WAL file.
    path: PathBuf,
    pub(super) min_id: Option<u64>,
    pub(super) max_id: Option<u64>,
}

impl Storage {
    /// Creates a new instance of [`Storage`] backed by the file at the given path and creates
    /// it doesn't exist.
    pub(super) fn new(path: impl AsRef<Path>) -> eyre::Result<Self> {
        reth_fs_util::create_dir_all(&path)?;

        let (mut min_id, mut max_id) = (None, None);

        for entry in reth_fs_util::read_dir(&path)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_id = Self::parse_filename(&file_name.to_string_lossy())?;

            min_id = min_id.map_or(Some(file_id), |min_id: u64| Some(min_id.min(file_id)));
            max_id = max_id.map_or(Some(file_id), |max_id: u64| Some(max_id.max(file_id)));
        }

        debug!(?min_id, ?max_id, "Initialized WAL storage");

        Ok(Self { path: path.as_ref().to_path_buf(), min_id, max_id })
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

    fn adjust_file_range(&self, range: impl RangeBounds<u64>) -> Option<RangeInclusive<u64>> {
        let (min_id, max_id) = self.min_id.zip(self.max_id)?;

        let start = match range.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => *start + 1,
            Bound::Unbounded => min_id,
        };
        let end = match range.end_bound() {
            Bound::Included(end) => *end,
            Bound::Excluded(end) => *end - 1,
            Bound::Unbounded => max_id,
        };

        Some(start..=end)
    }

    /// Removes notification for the given file ID from the storage.
    #[instrument(target = "exex::wal::storage", skip(self))]
    fn remove_notification(&self, file_id: u64) {
        match reth_fs_util::remove_file(self.file_path(file_id)) {
            Ok(()) => debug!("Notification was removed from the storage"),
            Err(err) => debug!(?err, "Failed to remove notification from the storage"),
        }
    }

    /// Removes notifications from the storage according to the given range.
    ///
    /// # Returns
    ///
    /// Number of removed notifications.
    pub(super) fn remove_notifications(
        &mut self,
        range: RemoveNotificationsRange,
    ) -> eyre::Result<usize> {
        let adjusted_range = match range {
            RemoveNotificationsRange::FromFileId(from_file_id) => {
                self.adjust_file_range(from_file_id..)
            }
            RemoveNotificationsRange::ToFileId(to_file_id) => self.adjust_file_range(..to_file_id),
        };
        let Some(adjusted_range) = adjusted_range else { return Ok(0) };

        for id in adjusted_range.clone() {
            self.remove_notification(id);
        }

        match range {
            RemoveNotificationsRange::FromFileId(from_file_id) => {
                self.max_id = from_file_id.checked_sub(1)
            }
            RemoveNotificationsRange::ToFileId(to_file_id) => self.min_id = Some(to_file_id),
        };

        Ok(adjusted_range.count())
    }

    /// Removes notifications from the storage according to the given range.
    ///
    /// # Returns
    ///
    /// Notifications that were removed.
    pub(super) fn take_notifications(
        &mut self,
        range: RemoveNotificationsRange,
    ) -> eyre::Result<Vec<ExExNotification>> {
        let adjusted_range = match range {
            RemoveNotificationsRange::FromFileId(from_file_id) => {
                self.adjust_file_range(from_file_id..)
            }
            RemoveNotificationsRange::ToFileId(to_file_id) => self.adjust_file_range(..to_file_id),
        };
        let Some(adjusted_range) = adjusted_range else { return Ok(Vec::new()) };

        let notifications =
            self.iter_notifications(adjusted_range).collect::<eyre::Result<Vec<_>>>()?;

        for (id, _) in &notifications {
            self.remove_notification(*id);
        }

        match range {
            RemoveNotificationsRange::FromFileId(from_file_id) => {
                self.max_id = from_file_id.checked_sub(1)
            }
            RemoveNotificationsRange::ToFileId(to_file_id) => self.min_id = Some(to_file_id),
        };

        Ok(notifications.into_iter().map(|(_, notification)| notification).collect())
    }

    pub(super) fn iter_notifications(
        &self,
        range: impl RangeBounds<u64>,
    ) -> Box<dyn Iterator<Item = eyre::Result<(u64, ExExNotification)>> + '_> {
        let Some(range) = self.adjust_file_range(range) else {
            return Box::new(std::iter::empty())
        };

        Box::new(
            range.map(move |id| self.read_notification(id).map(|notification| (id, notification))),
        )
    }

    /// Reads the notification from the file with the given id.
    pub(super) fn read_notification(&self, file_id: u64) -> eyre::Result<ExExNotification> {
        debug!(?file_id, "Reading notification from WAL");

        let file_path = self.file_path(file_id);
        let mut file = File::open(&file_path)?;
        read_notification(&mut file)
    }

    /// Writes the notification to the file with the given id.
    pub(super) fn write_notification(
        &mut self,
        notification: &ExExNotification,
    ) -> eyre::Result<u64> {
        let file_id = self.max_id.map_or(0, |id| id + 1);
        self.min_id = self.min_id.map_or(Some(file_id), |min_id| Some(min_id.min(file_id)));
        self.max_id = self.max_id.map_or(Some(file_id), |max_id| Some(max_id.max(file_id)));

        debug!(?file_id, "Writing notification to WAL");

        let file_path = self.file_path(file_id);
        let mut file = File::create_new(&file_path)?;
        write_notification(&mut file, notification)?;

        Ok(file_id)
    }
}

pub(super) enum RemoveNotificationsRange {
    /// Remove notifications starting from the given file ID, inclusive.
    FromFileId(u64),
    /// Remove notifications up to the given file ID, exclusive.
    ToFileId(u64),
}

fn write_notification(w: &mut impl Write, notification: &ExExNotification) -> eyre::Result<()> {
    rmp_serde::encode::write(w, notification)?;
    w.flush()?;
    Ok(())
}

fn read_notification(r: &mut impl Read) -> eyre::Result<ExExNotification> {
    Ok(rmp_serde::from_read(r)?)
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
        let mut storage = Storage::new(&temp_dir)?;

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
        let deserialized_notification = storage.read_notification(0)?;
        assert_eq!(deserialized_notification, notification);

        Ok(())
    }
}
