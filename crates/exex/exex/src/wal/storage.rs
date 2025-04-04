use std::{
    fs::File,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

use crate::wal::{WalError, WalResult};
use reth_ethereum_primitives::EthPrimitives;
use reth_exex_types::ExExNotification;
use reth_node_api::NodePrimitives;
use reth_tracing::tracing::debug;
use tracing::instrument;

static FILE_EXTENSION: &str = "wal";

/// The underlying WAL storage backed by a directory of files.
///
/// Each notification is represented by a single file that contains a MessagePack-encoded
/// notification.
#[derive(Debug, Clone)]
pub struct Storage<N: NodePrimitives = EthPrimitives> {
    /// The path to the WAL file.
    path: PathBuf,
    _pd: std::marker::PhantomData<N>,
}

impl<N> Storage<N>
where
    N: NodePrimitives,
{
    /// Creates a new instance of [`Storage`] backed by the file at the given path and creates
    /// it doesn't exist.
    pub(super) fn new(path: impl AsRef<Path>) -> WalResult<Self> {
        reth_fs_util::create_dir_all(&path)?;

        Ok(Self { path: path.as_ref().to_path_buf(), _pd: std::marker::PhantomData })
    }

    fn file_path(&self, id: u32) -> PathBuf {
        self.path.join(format!("{id}.{FILE_EXTENSION}"))
    }

    fn parse_filename(filename: &str) -> WalResult<u32> {
        filename
            .strip_suffix(".wal")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| WalError::Parse(filename.to_string()))
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
    pub(super) fn files_range(&self) -> WalResult<Option<RangeInclusive<u32>>> {
        let mut min_id = None;
        let mut max_id = None;

        for entry in reth_fs_util::read_dir(&self.path)? {
            let entry = entry.map_err(|err| WalError::DirEntry(self.path.clone(), err))?;

            if entry.path().extension() == Some(FILE_EXTENSION.as_ref()) {
                let file_name = entry.file_name();
                let file_id = Self::parse_filename(&file_name.to_string_lossy())?;

                min_id = min_id.map_or(Some(file_id), |min_id: u32| Some(min_id.min(file_id)));
                max_id = max_id.map_or(Some(file_id), |max_id: u32| Some(max_id.max(file_id)));
            }
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
    ) -> WalResult<(usize, u64)> {
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
    ) -> impl Iterator<Item = WalResult<(u32, u64, ExExNotification<N>)>> + '_ {
        range.map(move |id| {
            let (notification, size) =
                self.read_notification(id)?.ok_or(WalError::FileNotFound(id))?;

            Ok((id, size, notification))
        })
    }

    /// Reads the notification from the file with the given ID.
    #[instrument(skip(self))]
    pub(super) fn read_notification(
        &self,
        file_id: u32,
    ) -> WalResult<Option<(ExExNotification<N>, u64)>> {
        let file_path = self.file_path(file_id);
        debug!(target: "exex::wal::storage", ?file_path, "Reading notification from WAL");

        let mut file = match File::open(&file_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(reth_fs_util::FsPathError::open(err, &file_path).into()),
        };
        let size = file.metadata().map_err(|err| WalError::FileMetadata(file_id, err))?.len();

        // Deserialize using the bincode- and msgpack-compatible serde wrapper
        let notification: reth_exex_types::serde_bincode_compat::ExExNotification<'_, N> =
            rmp_serde::decode::from_read(&mut file)
                .map_err(|err| WalError::Decode(file_id, file_path, err))?;

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
        notification: &ExExNotification<N>,
    ) -> WalResult<u64> {
        let file_path = self.file_path(file_id);
        debug!(target: "exex::wal::storage", ?file_path, "Writing notification to WAL");

        // Serialize using the bincode- and msgpack-compatible serde wrapper
        let notification =
            reth_exex_types::serde_bincode_compat::ExExNotification::<N>::from(notification);

        reth_fs_util::atomic_write_file(&file_path, |file| {
            rmp_serde::encode::write(file, &notification)
        })?;

        Ok(file_path.metadata().map_err(|err| WalError::FileMetadata(file_id, err))?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use reth_exex_types::ExExNotification;
    use reth_provider::Chain;
    use reth_testing_utils::generators::{self, random_block};
    use std::{fs::File, sync::Arc};

    // wal with 1 block and tx
    // <https://github.com/paradigmxyz/reth/issues/15012>
    #[test]
    fn decode_notification_wal() {
        let wal = include_bytes!("../../test-data/28.wal");
        let notification: reth_exex_types::serde_bincode_compat::ExExNotification<
            '_,
            reth_ethereum_primitives::EthPrimitives,
        > = rmp_serde::decode::from_slice(wal.as_slice()).unwrap();
        let notification: ExExNotification = notification.into();
        match notification {
            ExExNotification::ChainCommitted { new } => {
                assert_eq!(new.blocks().len(), 1);
                assert_eq!(new.tip().transaction_count(), 1);
            }
            _ => panic!("unexpected notification"),
        }
    }

    #[test]
    fn test_roundtrip() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir()?;
        let storage: Storage = Storage::new(&temp_dir)?;

        let old_block = random_block(&mut rng, 0, Default::default()).try_recover()?;
        let new_block = random_block(&mut rng, 0, Default::default()).try_recover()?;

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

    #[test]
    fn test_files_range() -> eyre::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let storage: Storage = Storage::new(&temp_dir)?;

        // Create WAL files
        File::create(storage.file_path(1))?;
        File::create(storage.file_path(2))?;
        File::create(storage.file_path(3))?;

        // Create non-WAL files that should be ignored
        File::create(temp_dir.path().join("0.tmp"))?;
        File::create(temp_dir.path().join("4.tmp"))?;

        // Check files range
        assert_eq!(storage.files_range()?, Some(1..=3));

        Ok(())
    }
}
