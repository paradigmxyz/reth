use alloy_primitives::{keccak256, B256};
use eyre::Context;
use rayon::slice::ParallelSliceMut;
use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, Error as MdbxError, Geometry, Mode, SyncMode,
    WriteFlags, RO,
};
use reth_provider::{DBProvider, ExecutionOutcome};
use reth_revm::revm::database::states::RevertToSlot;
use reth_stages_api::StageError;
use std::{collections::HashSet, path::Path};
use tracing::trace;

/// Separate MDBX environment for storing `keccak256(slot) → slot` preimage mappings.
///
/// Used only during [`super::ExecutionStage`] for pre-Cancun selfdestruct handling where
/// the original storage slot keys must be recovered from their hashed representation.
///
/// The database is append-only and not unwound — duplicate inserts are silently skipped.
/// After Cancun (where `SELFDESTRUCT` no longer destroys storage) the database can be pruned.
#[derive(Debug)]
struct SlotPreimages {
    env: Environment,
}

impl SlotPreimages {
    /// Opens (or creates) the slot-preimage MDBX environment at the given `path`.
    ///
    /// Uses the same environment settings as the main reth MDBX database (WriteMap mode,
    /// matching geometry and page size).
    fn open(path: &Path) -> eyre::Result<Self> {
        const GIGABYTE: usize = 1024 * 1024 * 1024;
        const TERABYTE: usize = GIGABYTE * 1024;

        let mut builder = Environment::builder();
        builder.set_max_dbs(1);
        let os_page_size = page_size::get().clamp(4096, 0x10000);
        builder.set_geometry(Geometry {
            size: Some(0..(8 * TERABYTE)),
            growth_step: Some(4 * GIGABYTE as isize),
            shrink_threshold: Some(0),
            page_size: Some(reth_libmdbx::PageSize::Set(os_page_size)),
        });
        builder.write_map();
        builder.set_flags(EnvironmentFlags {
            mode: Mode::ReadWrite { sync_mode: SyncMode::Durable },
            ..Default::default()
        });

        let env = builder.open(path).wrap_err_with(|| {
            format!("failed to open slot-preimage MDBX env at {}", path.display())
        })?;

        // Ensure the unnamed default DB exists.
        {
            let tx = env.begin_rw_txn()?;
            let _db = tx.create_db(None, DatabaseFlags::empty())?;
            tx.commit()?;
        }

        trace!(target: "stages::slot_preimages", ?path, "Opened slot-preimage store");

        Ok(Self { env })
    }

    /// Batch-insert `hashed_slot → plain_slot` preimage entries.
    ///
    /// Entries must be pre-sorted by key for optimal insert performance.
    /// Duplicate keys are silently skipped via [`WriteFlags::NO_OVERWRITE`].
    fn insert_preimages(&self, entries: &[(B256, B256)]) -> eyre::Result<()> {
        let tx = self.env.begin_rw_txn()?;
        let db = tx.open_db(None)?;

        for (hashed_slot, plain_slot) in entries {
            match tx.put(
                db.dbi(),
                hashed_slot.as_slice(),
                plain_slot.as_slice(),
                WriteFlags::NO_OVERWRITE,
            ) {
                Ok(()) => {}
                Err(MdbxError::KeyExist) => {}
                Err(e) => return Err(e.into()),
            }
        }

        tx.commit()?;

        trace!(target: "stages::slot_preimages", count = entries.len(), "Inserted slot preimages");

        Ok(())
    }

    /// Opens a read-only transaction for batch lookups.
    ///
    /// Reuse the returned [`SlotPreimagesReader`] for multiple `get` calls to avoid
    /// the overhead of opening a new RO transaction per lookup.
    fn reader(&self) -> eyre::Result<SlotPreimagesReader> {
        let tx = self.env.begin_ro_txn()?;
        let dbi = tx.open_db(None)?.dbi();
        Ok(SlotPreimagesReader { tx, dbi })
    }
}

/// Read-only handle for batch slot-preimage lookups within a single MDBX transaction.
struct SlotPreimagesReader {
    tx: reth_libmdbx::Transaction<RO>,
    dbi: reth_libmdbx::ffi::MDBX_dbi,
}

impl SlotPreimagesReader {
    /// Point-lookup of a slot preimage by its keccak256 hash.
    fn get(&self, hashed_slot: &B256) -> eyre::Result<Option<B256>> {
        let result: Option<[u8; 32]> = self.tx.get(self.dbi, hashed_slot.as_ref())?;
        Ok(result.map(B256::from))
    }
}

/// Collects `keccak256(slot) → slot` preimage entries from the bundle state and stores
/// them in the auxiliary preimage database, then rewrites wipe reverts for self-destructed
/// accounts to use plain slot keys instead of relying on the hashed-storage DB walk.
///
/// This eliminates the need for the changeset writer to read from `HashedStorages` during
/// storage wipes, keeping all changeset keys in plain format.
pub(super) fn inject_plain_wipe_slots<P: DBProvider, R>(
    slot_preimages_path: &Path,
    provider: &P,
    state: &mut ExecutionOutcome<R>,
) -> Result<(), StageError> {
    // Collect preimage entries from all storage changes in the bundle.
    // StorageKey in revm is U256, representing a plain EVM slot index.
    let mut preimage_entries = Vec::new();
    let mut seen_hashes = HashSet::new();
    for (_, account) in state.bundle.state() {
        for (&slot_key, _) in account.storage.iter() {
            let plain = B256::from(slot_key.to_be_bytes());
            let hashed = keccak256(plain);
            if seen_hashes.insert(hashed) {
                preimage_entries.push((hashed, plain));
            }
        }
    }

    // Pre-sort entries by hash key for optimal MDBX insert performance.
    preimage_entries.par_sort_unstable_by_key(|(hash, _)| *hash);

    // Lazily open the preimage store and insert entries.
    let preimages = SlotPreimages::open(slot_preimages_path)
        .map_err(|e| StageError::Fatal(Box::new(std::io::Error::other(e.to_string()))))?;

    if !preimage_entries.is_empty() {
        preimages
            .insert_preimages(&preimage_entries)
            .map_err(|e| StageError::Fatal(Box::new(std::io::Error::other(e.to_string()))))?;
    }

    // Find all wipe reverts (self-destructed accounts) and inject plain slot keys.
    // Track addresses already wiped within this batch to handle the case where an
    // account is destroyed, re-created, and destroyed again in the same batch.
    let mut already_wiped = HashSet::new();

    // Open a single RO transaction for all preimage lookups in this batch.
    let reader = preimages
        .reader()
        .map_err(|e| StageError::Fatal(Box::new(std::io::Error::other(e.to_string()))))?;

    for block_reverts in state.bundle.reverts.iter_mut() {
        for (address, revert) in block_reverts.iter_mut() {
            if !revert.wipe_storage {
                continue;
            }

            if !already_wiped.insert(*address) {
                // Second (or subsequent) destruction within the same batch: skip the DB walk.
                // All slots from re-creation are already in `revert.storage` as explicit
                // changes from execution.
                continue;
            }

            // Walk all hashed storage slots for this account in the DB and look up
            // their plain-key preimages.
            let addr = *address;
            let hashed_address = keccak256(addr);
            let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>()?;

            if let Some((_, entry)) = cursor.seek_exact(hashed_address)? {
                inject_preimage_entry(&reader, revert, addr, entry.key, entry.value)?;
                while let Some(entry) = cursor.next_dup_val()? {
                    inject_preimage_entry(&reader, revert, addr, entry.key, entry.value)?;
                }
            }
        }
    }

    Ok(())
}

/// Looks up the plain-key preimage for a single hashed storage slot and inserts it
/// into the account revert if not already present.
fn inject_preimage_entry(
    reader: &SlotPreimagesReader,
    revert: &mut reth_revm::revm::database::AccountRevert,
    address: alloy_primitives::Address,
    hashed_slot: B256,
    value: alloy_primitives::U256,
) -> Result<(), StageError> {
    let plain_slot = reader
        .get(&hashed_slot)
        .map_err(|e| StageError::Fatal(Box::new(std::io::Error::other(e.to_string()))))?
        .ok_or_else(|| {
            StageError::Fatal(Box::new(std::io::Error::other(format!(
                "missing slot preimage for {hashed_slot:?} (addr={address:?})"
            ))))
        })?;

    // Convert B256 plain slot to U256 StorageKey for the revert map.
    let plain_key = alloy_primitives::U256::from_be_bytes(plain_slot.0);
    revert.storage.entry(plain_key).or_insert(RevertToSlot::Some(value));
    Ok(())
}
