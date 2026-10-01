//! Offline, byte-preserving migration from the two state trie families to one.

use super::{RocksDBBuilder, RocksDBProvider};
use itertools::Itertools;
use reth_db_api::tables;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use rocksdb::{Cache, IngestExternalFileOptions, SstFileWriter};

type Entry = ([u8; 66], usize, Box<[u8]>);

impl RocksDBProvider {
    /// Copies the two existing state trie families into an empty unified family.
    /// The caller must stop the node and verify the durable state frontier first.
    /// Existing families are only read; values are copied without re-encoding.
    pub fn migrate_unified_state_trie(&self) -> ProviderResult<u64> {
        if self.raw_iter::<tables::UnifiedStateTrieAccounts>()?.next().transpose()?.is_some() {
            return Err(ProviderError::other(std::io::Error::other(
                "StateTrie must be empty before migration",
            )))
        }
        let dir = tempfile::Builder::new()
            .prefix("unified-migration-")
            .tempdir_in(self.0.path())
            .map_err(ProviderError::other)?;
        let path = dir.path().join("state.sst");
        let options =
            RocksDBBuilder::state_trie_column_family_options(&Cache::new_lru_cache(0), false);
        let mut ingest = IngestExternalFileOptions::default();
        ingest.set_move_files(true);
        let db = self.0.db_rw();
        let cf = self.get_cf_handle::<tables::UnifiedStateTrieAccounts>()?;
        let mut writer = None;
        let mut count = 0;
        for entry in self.unified_migration_source()? {
            let (key, len, value) = entry?;
            if writer.is_none() {
                let next = SstFileWriter::create(&options);
                next.open(&path).map_err(ProviderError::other)?;
                writer = Some(next);
            }
            let current = writer.as_mut().expect("opened SST");
            current.put(&key[..len], &value).map_err(ProviderError::other)?;
            count += 1;
            if current.file_size() >= 256 << 20 {
                writer.take().expect("opened SST").finish().map_err(ProviderError::other)?;
                db.ingest_external_file_cf_opts(cf, &ingest, vec![&path])
                    .map_err(ProviderError::other)?;
                eprintln!("migrated_nodes={count}");
            }
        }
        if let Some(mut writer) = writer {
            writer.finish().map_err(ProviderError::other)?;
            db.ingest_external_file_cf_opts(cf, &ingest, vec![&path])
                .map_err(ProviderError::other)?;
        }
        Ok(count)
    }

    /// Compares every unified key and value with the unchanged source families.
    /// This is an offline migration check, before the unified trie receives new blocks.
    pub fn verify_unified_state_trie_migration(&self) -> ProviderResult<u64> {
        let mut actual = self.raw_iter::<tables::UnifiedStateTrieAccounts>()?;
        let mut count = 0;
        for entry in self.unified_migration_source()? {
            let (key, len, value) = entry?;
            let Some((actual_key, actual_value)) = actual.next().transpose()? else {
                return Err(ProviderError::other(std::io::Error::other(
                    "unified trie is missing source records",
                )))
            };
            if actual_key.as_ref() != &key[..len] || actual_value != value {
                return Err(ProviderError::other(std::io::Error::other(format!(
                    "unified trie differs at record {count}"
                ))))
            }
            count += 1;
            if count % 100_000_000 == 0 {
                eprintln!("verified_nodes={count}");
            }
        }
        if actual.next().transpose()?.is_some() {
            return Err(ProviderError::other(std::io::Error::other(
                "unified trie contains extra records",
            )))
        }
        Ok(count)
    }

    fn unified_migration_source(
        &self,
    ) -> ProviderResult<impl Iterator<Item = ProviderResult<Entry>> + '_> {
        let accounts = self.raw_iter::<tables::StateTrieAccounts>()?.map(|entry| {
            let (source, value) = entry?;
            if source.len() != 33 || source[32] > 64 {
                return Err(ProviderError::other(std::io::Error::other(
                    "invalid source account path",
                )))
            }
            let mut key = [0; 66];
            key[..33].copy_from_slice(&source);
            Ok((key, 33, value))
        });
        let storages = self.raw_iter::<tables::RocksStateTrieStorages>()?.map(|entry| {
            let (source, value) = entry?;
            if source.len() != 65 || source[64] > 64 {
                return Err(ProviderError::other(std::io::Error::other(
                    "invalid source storage path",
                )))
            }
            let mut key = [0; 66];
            key[..32].copy_from_slice(&source[..32]);
            key[32] = 64;
            key[33..].copy_from_slice(&source[32..]);
            Ok((key, 66, value))
        });
        Ok(accounts.merge_by(storages, |a, b| match (a, b) {
            (Ok((a, alen, _)), Ok((b, blen, _))) => a[..*alen] <= b[..*blen],
            _ => true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, U256};
    use reth_db_api::{models::state_trie::StateTrieStorageKey, table::Table};
    use reth_trie::{
        state_trie_cursor::{StateTrieCursor, StateTrieCursorFactory},
        Nibbles, StateTrieNode, TrieAccount,
    };

    #[test]
    fn unified_migration_preserves_sources_and_detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = RocksDBProvider::builder(dir.path()).with_default_tables().build().unwrap();
        let account = StateTrieNode::Leaf { short_key_len: 64, value: TrieAccount::default() };
        let slot = StateTrieNode::Leaf { short_key_len: 64, value: U256::from(3) };
        let addresses = [B256::ZERO, B256::with_last_byte(1), B256::repeat_byte(255)];
        for address in addresses {
            db.put::<tables::StateTrieAccounts>(Nibbles::unpack(address).into(), &account).unwrap();
            for path in [
                Nibbles::new(),
                Nibbles::unpack(B256::ZERO),
                Nibbles::unpack(B256::repeat_byte(255)),
            ] {
                db.put::<tables::RocksStateTrieStorages>(
                    StateTrieStorageKey { address, path: path.into() },
                    &slot,
                )
                .unwrap();
            }
        }
        let accounts: Vec<_> =
            db.raw_iter::<tables::StateTrieAccounts>().unwrap().collect::<Result<_, _>>().unwrap();
        let storages: Vec<_> = db
            .raw_iter::<tables::RocksStateTrieStorages>()
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(db.migrate_unified_state_trie().unwrap(), 12);
        assert_eq!(db.verify_unified_state_trie_migration().unwrap(), 12);
        assert!(db.migrate_unified_state_trie().is_err());
        assert_eq!(
            db.raw_iter::<tables::StateTrieAccounts>()
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            accounts
        );
        assert_eq!(
            db.raw_iter::<tables::RocksStateTrieStorages>()
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            storages
        );
        let snapshot = db.snapshot();
        let mut cursor = snapshot.state_trie_account_cursor().unwrap();
        assert_eq!(
            cursor.before(None).unwrap(),
            Some((Nibbles::unpack(addresses[2]), account.clone()))
        );
        assert_eq!(
            cursor.before(Some(Nibbles::unpack(addresses[1]))).unwrap(),
            Some((Nibbles::unpack(addresses[0]), account.clone()))
        );
        assert_eq!(
            cursor.seek(Nibbles::unpack(B256::with_last_byte(2))).unwrap(),
            Some((Nibbles::unpack(addresses[2]), account))
        );
        db.delete::<tables::UnifiedStateTrieAccounts>(Nibbles::unpack(addresses[0]).into())
            .unwrap();
        db.flush(&[tables::UnifiedStateTrieAccounts::NAME]).unwrap();
        assert!(db.verify_unified_state_trie_migration().is_err());
    }
}
