//! Persists contract code ahead of the account range that commits to it.
//!
//! Code is content addressed, so a blob is authenticated by its hash alone and stays valid however
//! far the pivot moves. Accounts sharing a code hash therefore need it downloaded and stored once.

use crate::{SnapAttemptStore, SnapSyncError, SnapWrite};
use alloy_primitives::{keccak256, map::B256Set, Bytes, B256, KECCAK256_EMPTY};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable,
};
use reth_storage_api::{DBProvider, MetadataProvider, StateWriter};
use reth_trie_common::TrieAccount;
use revm::{bytecode::Bytecode, database::states::StateChangeset};

/// Persistence for the code an attempt's accounts reference.
///
/// Blanket-implemented over the node's writers, so code joins the caller's transaction and commits
/// with whatever else that transaction carries.
pub trait SnapBytecodeStore {
    /// Returns the code hashes `accounts` reference that are not stored yet, once each and in the
    /// order the accounts reference them.
    fn missing_code(
        &self,
        write: SnapWrite,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<Vec<B256>, SnapSyncError>
    where
        Self: DBProvider;

    /// Persists `codes`, each checked against the hash it was downloaded under, and returns how
    /// many were written.
    fn commit_bytecodes(
        &self,
        write: SnapWrite,
        codes: Vec<(B256, Bytes)>,
    ) -> Result<usize, SnapSyncError>
    where
        Self: StateWriter + DBProvider<Tx: DbTxMut>;
}

impl<T: MetadataProvider> SnapBytecodeStore for T {
    fn missing_code(
        &self,
        write: SnapWrite,
        accounts: &[(B256, TrieAccount)],
    ) -> Result<Vec<B256>, SnapSyncError>
    where
        Self: DBProvider,
    {
        self.authorize_snap_write(write)?;
        let mut seen = B256Set::default();
        let mut missing = Vec::new();
        for (_, account) in accounts {
            // Only presence matters, so stored code is not decoded.
            if account.code_hash != KECCAK256_EMPTY &&
                seen.insert(account.code_hash) &&
                self.tx_ref()
                    .get::<RawTable<tables::Bytecodes>>(RawKey::new(account.code_hash))?
                    .is_none()
            {
                missing.push(account.code_hash);
            }
        }
        Ok(missing)
    }

    // Every check runs before the first write, so a refused response changes nothing.
    fn commit_bytecodes(
        &self,
        write: SnapWrite,
        codes: Vec<(B256, Bytes)>,
    ) -> Result<usize, SnapSyncError>
    where
        Self: StateWriter + DBProvider<Tx: DbTxMut>,
    {
        self.authorize_snap_write(write)?;
        let contracts = codes
            .into_iter()
            .map(|(hash, code)| {
                let got = keccak256(&code);
                if got != hash {
                    return Err(SnapSyncError::CodeMismatch { expected: hash, got })
                }
                // Code deployed before EIP-3541 can carry the delegation prefix without being
                // one, so authenticated bytes that do not parse as a delegation are legacy code.
                let code = Bytecode::new_raw_checked(code.clone())
                    .unwrap_or_else(|_| Bytecode::new_legacy(code));
                Ok((hash, code))
            })
            .collect::<Result<Vec<_>, SnapSyncError>>()?;

        let written = contracts.len();
        self.write_state_changes(StateChangeset { contracts, ..Default::default() })?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{account, generation, hashed_factory, key, state_root};
    use alloy_primitives::bytes;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, DatabaseProviderFactory, ProviderFactory,
    };

    type Factory = ProviderFactory<MockNodeTypesWithDB>;

    fn code(byte: u8) -> Bytes {
        Bytes::from(vec![byte; 4])
    }

    // An account referencing `code`, distinguished by its nonce.
    fn contract(nonce: u64, code: &Bytes) -> TrieAccount {
        let mut contract = account(nonce);
        contract.code_hash = keccak256(code);
        contract
    }

    fn started(accounts: &[(B256, TrieAccount)]) -> (Factory, SnapWrite) {
        let factory = hashed_factory();
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(generation(1, state_root(accounts))).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    fn is_stored(provider: &impl DBProvider, code: &Bytes) -> bool {
        provider.tx_ref().get::<tables::Bytecodes>(keccak256(code)).unwrap().is_some()
    }

    #[test]
    fn accounts_sharing_code_need_it_downloaded_once() {
        let shared = code(1);
        let accounts = vec![
            (key(1), contract(1, &shared)),
            (key(2), account(2)),
            (key(3), contract(3, &shared)),
            (key(4), contract(4, &code(2))),
        ];
        let (factory, write) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();

        let missing = provider.missing_code(write, &accounts).unwrap();

        assert_eq!(missing, [keccak256(&shared), keccak256(code(2))]);
        assert_eq!(
            provider.commit_bytecodes(write, vec![(missing[0], shared.clone())]).unwrap(),
            1
        );
        // The shared blob answers both accounts, leaving only the other contract's code.
        assert_eq!(provider.missing_code(write, &accounts).unwrap(), [keccak256(code(2))]);
        assert!(is_stored(&provider, &shared));
    }

    #[test]
    fn code_already_stored_is_not_requested_again() {
        let stored = code(1);
        let accounts = vec![(key(1), contract(1, &stored))];
        let (factory, write) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        provider.commit_bytecodes(write, vec![(keccak256(&stored), stored.clone())]).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert!(provider.missing_code(write, &accounts).unwrap().is_empty());
        assert!(is_stored(&provider, &stored));
    }

    #[test]
    fn code_that_does_not_hash_to_its_requested_hash_is_refused() {
        let wanted = code(1);
        let accounts = vec![(key(1), contract(1, &wanted))];
        let (factory, write) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        let hash = keccak256(&wanted);

        let refused = provider.commit_bytecodes(write, vec![(hash, code(2))]);

        assert!(
            matches!(refused, Err(SnapSyncError::CodeMismatch { expected, .. }) if expected == hash)
        );
        assert_eq!(provider.missing_code(write, &accounts).unwrap(), [hash]);
        assert!(!is_stored(&provider, &wanted));
    }

    // A delegation is exactly 23 bytes, but EIP-3541 only bars the prefix from London onwards, so
    // an older account can reference shorter code that starts with it.
    #[test]
    fn delegation_shaped_code_that_predates_eip_3541_is_kept_as_legacy_code() {
        let historical = bytes!("ef0100");
        let accounts = vec![(key(1), contract(1, &historical))];
        let (factory, write) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();

        let hash = keccak256(&historical);
        assert_eq!(provider.commit_bytecodes(write, vec![(hash, historical.clone())]).unwrap(), 1);

        let stored = provider.tx_ref().get::<tables::Bytecodes>(hash).unwrap().unwrap();
        assert_eq!(stored.original_bytes(), historical);
        assert!(provider.missing_code(write, &accounts).unwrap().is_empty());
    }

    #[test]
    fn code_is_refused_once_the_attempt_no_longer_owns_the_state() {
        let wanted = code(1);
        let accounts = vec![(key(1), contract(1, &wanted))];
        let (factory, write) = started(&accounts);
        let provider = factory.database_provider_rw().unwrap();
        provider.advance_snap_pivot(write, generation(2, B256::repeat_byte(0xcc))).unwrap();

        let refused = provider.commit_bytecodes(write, vec![(keccak256(&wanted), wanted.clone())]);

        assert!(matches!(refused, Err(SnapSyncError::StaleWrite { .. })));
        assert!(matches!(
            provider.missing_code(write, &accounts),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        assert!(!is_stored(&provider, &wanted));
    }
}
