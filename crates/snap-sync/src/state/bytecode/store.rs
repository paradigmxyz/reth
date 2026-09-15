//! Validates bytecode dependencies and writes them within the owning state transaction.

use crate::SnapSyncError;
use alloy_primitives::{map::B256Set, B256, KECCAK256_EMPTY};
use reth_db_api::{tables, transaction::DbTx, RawKey, RawTable};
use reth_storage_api::StateWriter;
use reth_storage_errors::provider::ProviderError;
use revm::{bytecode::Bytecode, database::states::StateChangeset};

// Hashes of the supplied code, refusing code filed under a hash it does not hash to.
pub(crate) fn supplied_code(bytecodes: &[(B256, Bytecode)]) -> Result<B256Set, SnapSyncError> {
    let mut hashes = B256Set::default();
    for (hash, code) in bytecodes {
        let got = code.hash_slow();
        if got != *hash {
            return Err(SnapSyncError::CodeMismatch { expected: *hash, got })
        }
        hashes.insert(*hash);
    }
    Ok(hashes)
}

// Persists code in the transaction that owns the account or BAL update.
pub(crate) fn write_bytecodes(
    writer: &impl StateWriter,
    contracts: Vec<(B256, Bytecode)>,
) -> Result<(), ProviderError> {
    writer.write_state_changes(StateChangeset { contracts, ..Default::default() })
}

// Only presence matters, so stored code is not decoded.
pub(crate) fn require_code(
    tx: &impl DbTx,
    available: &mut B256Set,
    hash: B256,
) -> Result<(), SnapSyncError> {
    if hash != KECCAK256_EMPTY &&
        available.insert(hash) &&
        tx.get::<RawTable<tables::Bytecodes>>(RawKey::new(hash))?.is_none()
    {
        return Err(SnapSyncError::MissingCode { hash })
    }
    Ok(())
}
