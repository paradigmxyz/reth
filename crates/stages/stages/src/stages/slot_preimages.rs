use alloy_primitives::B256;
use eyre::Context;
use reth_libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, Error as MdbxError, Geometry, Mode, SyncMode,
    WriteFlags, RO,
};
use std::path::Path;
use tracing::trace;

/// Separate MDBX environment for storing `keccak256(slot) → slot` preimage mappings.
///
/// Used only during [`super::ExecutionStage`] for pre-Cancun selfdestruct handling where
/// the original storage slot keys must be recovered from their hashed representation.
///
/// The database is append-only and not unwound — duplicate inserts are silently skipped.
/// After Cancun (where `SELFDESTRUCT` no longer destroys storage) the database can be pruned.
#[derive(Debug)]
pub(super) struct SlotPreimages {
    env: Environment,
}

impl SlotPreimages {
    /// Opens (or creates) the slot-preimage MDBX environment at the given `path`.
    ///
    /// Uses `WriteMap` mode for performance. The geometry allows growth up to 64 GB to
    /// accommodate chains with many unique storage keys (mapping-derived keys especially).
    pub(super) fn open(path: &Path) -> eyre::Result<Self> {
        let mut builder = Environment::builder();
        builder.set_max_dbs(1);
        builder.set_geometry(Geometry {
            size: Some(256 * 1024..64 * 1024 * 1024 * 1024),
            ..Default::default()
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
    /// Duplicate keys are silently skipped via [`WriteFlags::NO_OVERWRITE`].
    pub(super) fn insert_preimages(&self, entries: &[(B256, B256)]) -> eyre::Result<()> {
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
    pub(super) fn reader(&self) -> eyre::Result<SlotPreimagesReader> {
        let tx = self.env.begin_ro_txn()?;
        let dbi = tx.open_db(None)?.dbi();
        Ok(SlotPreimagesReader { tx, dbi })
    }
}

/// Read-only handle for batch slot-preimage lookups within a single MDBX transaction.
pub(super) struct SlotPreimagesReader {
    tx: reth_libmdbx::Transaction<RO>,
    dbi: reth_libmdbx::ffi::MDBX_dbi,
}

impl SlotPreimagesReader {
    /// Point-lookup of a slot preimage by its keccak256 hash.
    pub(super) fn get(&self, hashed_slot: &B256) -> eyre::Result<Option<B256>> {
        let result: Option<[u8; 32]> = self.tx.get(self.dbi, hashed_slot.as_ref())?;
        Ok(result.map(B256::from))
    }
}
