use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
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

    /// Returns the file IDs in the storage in ascending order.
    pub(super) fn file_ids(&self) -> WalResult<Vec<u32>> {
        let mut file_ids = Vec::new();

        for entry in reth_fs_util::read_dir(&self.path)? {
            let entry = entry.map_err(|err| WalError::DirEntry(self.path.clone(), err))?;

            if entry.path().extension() == Some(FILE_EXTENSION.as_ref()) {
                let file_name = entry.file_name();
                let file_id = Self::parse_filename(&file_name.to_string_lossy())?;
                file_ids.push(file_id);
            }
        }

        file_ids.sort_unstable();
        Ok(file_ids)
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

    pub(super) fn iter_notifications<'a>(
        &'a self,
        file_ids: impl IntoIterator<Item = u32> + 'a,
    ) -> impl Iterator<Item = WalResult<(u32, u64, ExExNotification<N>)>> + 'a {
        file_ids.into_iter().map(move |id| {
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
            rmp_serde::decode::from_read(BufReader::new(&mut file))
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
            let mut writer = BufWriter::new(file);
            rmp_serde::encode::write(&mut writer, &notification)?;
            // a `BufWriter` dropped without an explicit flush discards write errors, and
            // `atomic_write_file` fsyncs as soon as this returns
            writer.flush()?;
            Ok::<_, Box<dyn core::error::Error + Send + Sync>>(())
        })?;

        Ok(file_path.metadata().map_err(|err| WalError::FileMetadata(file_id, err))?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use alloy_consensus::BlockHeader;
    use alloy_primitives::{
        map::{HashMap, HashSet},
        B256, U256,
    };
    use reth_exex_types::ExExNotification;
    use reth_primitives_traits::Account;
    use reth_provider::Chain;
    use reth_testing_utils::generators::{self, random_block};
    use reth_trie_common::{
        serde_bincode_compat,
        updates::{StorageTrieUpdates, StorageTrieUpdatesSorted, TrieUpdates},
        BranchNodeCompact, ComputedTrieData, HashedPostState, HashedStorage, HashedStorageSorted,
        LazyTrieData, Nibbles,
    };
    use std::{collections::BTreeMap, fs::File, sync::Arc};

    #[test]
    fn test_roundtrip() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir()?;
        let storage: Storage = Storage::new(&temp_dir)?;

        let old_block = random_block(&mut rng, 0, Default::default()).try_recover()?;
        let new_block = random_block(&mut rng, 0, Default::default()).try_recover()?;

        let notification = ExExNotification::ChainReorged {
            new: Arc::new(Chain::new(vec![new_block], Default::default(), BTreeMap::new())),
            old: Arc::new(Chain::new(vec![old_block], Default::default(), BTreeMap::new())),
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
    fn test_decode_legacy_sorted_trie_data() -> eyre::Result<()> {
        let storage_nodes =
            vec![(Nibbles::from_nibbles_unchecked([0x01]), Some(BranchNodeCompact::default()))];
        let encoded = rmp_serde::encode::to_vec(&(false, &storage_nodes))?;
        let decoded: serde_bincode_compat::updates::StorageTrieUpdatesSorted<'_> =
            rmp_serde::decode::from_slice(&encoded)?;
        let decoded: StorageTrieUpdatesSorted = decoded.into();
        assert_eq!(decoded.storage_nodes, storage_nodes);

        let storage_slots = vec![(B256::from([1; 32]), U256::from(1))];
        let encoded = rmp_serde::encode::to_vec(&(&storage_slots, false))?;
        let decoded: serde_bincode_compat::hashed_state::HashedStorageSorted<'_> =
            rmp_serde::decode::from_slice(&encoded)?;
        let decoded: HashedStorageSorted = decoded.into();
        assert_eq!(decoded.storage_slots, storage_slots);

        Ok(())
    }

    /// Generate a new WAL file for testing.
    ///
    /// Run this test with `--ignored` to generate a new test WAL file:
    /// ```sh
    /// cargo test -p reth-exex generate_test_wal -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn generate_test_wal() -> eyre::Result<()> {
        use std::io::Write;

        let notification = get_test_notification_data()?;

        // Serialize the notification
        let notification_compat =
            reth_exex_types::serde_bincode_compat::ExExNotification::from(&notification);
        let encoded = rmp_serde::encode::to_vec(&notification_compat)?;

        // Write to test-data directory
        let test_data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
        std::fs::create_dir_all(&test_data_dir)?;

        let output_path = test_data_dir.join("new_format.wal");
        let mut file = File::create(&output_path)?;
        file.write_all(&encoded)?;

        println!("Generated WAL file at: {}", output_path.display());
        println!("File size: {} bytes", encoded.len());
        println!("✓ WAL file created successfully!");

        Ok(())
    }

    /// Helper function to generate deterministic test data for WAL tests
    fn get_test_notification_data(
    ) -> eyre::Result<ExExNotification<reth_ethereum_primitives::EthPrimitives>> {
        use reth_ethereum_primitives::Block;
        use reth_primitives_traits::Block as _;

        // Create a block with a transaction
        let block = Block::default().seal_slow().try_recover()?;
        let block_number = block.header().number();

        let hashed_address = B256::from([1; 32]);
        let storage_key = B256::from([2; 32]);

        let trie_updates = TrieUpdates {
            account_nodes: HashMap::from_iter([
                (Nibbles::from_nibbles_unchecked([0x01]), BranchNodeCompact::default()),
                (Nibbles::from_nibbles_unchecked([0x02]), BranchNodeCompact::default()),
            ]),
            removed_nodes: HashSet::from_iter([Nibbles::from_nibbles_unchecked([0x03])]),
            storage_tries: HashMap::from_iter([(
                hashed_address,
                StorageTrieUpdates {
                    storage_nodes: HashMap::from_iter([(
                        Nibbles::from_nibbles_unchecked([0x04]),
                        BranchNodeCompact::default(),
                    )]),
                    removed_nodes: Default::default(),
                },
            )]),
        };

        let hashed_state = HashedPostState {
            accounts: HashMap::from_iter([(
                hashed_address,
                Some(Account { nonce: 1, ..Default::default() }),
            )]),
            storages: HashMap::from_iter([(
                hashed_address,
                HashedStorage { storage: HashMap::from_iter([(storage_key, U256::from(101))]) },
            )]),
        };

        let trie_data = LazyTrieData::ready(ComputedTrieData::new(
            Arc::new(hashed_state.into_sorted()),
            Arc::new(trie_updates.into_sorted()),
        ));

        let notification: ExExNotification<reth_ethereum_primitives::EthPrimitives> =
            ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![block],
                    Default::default(),
                    BTreeMap::from([(block_number, trie_data)]),
                )),
            };
        Ok(notification)
    }

    #[test]
    fn test_file_ids() -> eyre::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let storage: Storage = Storage::new(&temp_dir)?;

        // Create WAL files
        File::create(storage.file_path(1))?;
        File::create(storage.file_path(3))?;

        // Create non-WAL files that should be ignored
        File::create(temp_dir.path().join("0.tmp"))?;
        File::create(temp_dir.path().join("4.tmp"))?;

        // Check existing file IDs are returned in order without filling the gap
        assert_eq!(storage.file_ids()?, vec![1, 3]);

        Ok(())
    }
}
