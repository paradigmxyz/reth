//! Helper types to workaround 'higher-ranked lifetime error'
//! <https://github.com/rust-lang/rust/issues/100013> in default implementation of
//! `reth_rpc_eth_api::helpers::Call`.

use alloy_eip7928::{bal::DecodedBal, BlockAccessIndex};
use alloy_primitives::{Address, B256, U256};
use reth_errors::ProviderResult;
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{BytecodeReader, HashedPostStateProvider, StateProvider, StateProviderBox};
use reth_trie::{HashedStorage, MultiProofTargets};
use revm::{
    database::{BundleState, State},
    state::bal::Bal as RevmBal,
    Database,
};
use std::sync::Arc;

/// Helper alias type for the state's [`State`]
pub type StateCacheDb = State<StateProviderDatabase<StateProviderTraitObjWrapper>>;

/// Attaches `bal` to the database, positioned at the state right before the transaction at
/// `tx_index`.
///
/// Reads served by the attached BAL reflect all writes prior to the transaction, including the
/// block's pre-execution system calls. Reads not covered by the BAL fall back to the underlying
/// database, which holds the correct values for all state the block does not touch.
///
/// Note: changes must not be committed to the database afterwards, because the attached BAL takes
/// precedence over committed state when serving reads.
#[inline]
pub fn attach_bal_before_tx<DB: Database>(
    db: &mut State<DB>,
    bal: &DecodedBal<Arc<RevmBal>>,
    tx_index: usize,
) {
    db.set_bal(Some(bal.as_bal().clone()));
    db.set_allow_bal_db_fallback(true);
    db.set_bal_index(BlockAccessIndex::from_tx_index(tx_index as u64));
}

/// Hack to get around 'higher-ranked lifetime error', see
/// <https://github.com/rust-lang/rust/issues/100013>
///
/// Apparently, when dealing with our RPC code, compiler is struggling to prove lifetimes around
/// [`StateProvider`] trait objects. This type is a workaround which should help the compiler to
/// understand that there are no lifetimes involved.
#[expect(missing_debug_implementations)]
pub struct StateProviderTraitObjWrapper(pub StateProviderBox);

impl reth_storage_api::StateRootProvider for StateProviderTraitObjWrapper {
    fn state_root(
        &self,
        hashed_state: reth_trie::HashedPostState,
    ) -> reth_errors::ProviderResult<B256> {
        self.0.state_root(hashed_state)
    }

    fn state_root_from_nodes(
        &self,
        input: reth_trie::TrieInput,
    ) -> reth_errors::ProviderResult<B256> {
        self.0.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: reth_trie::HashedPostState,
    ) -> reth_errors::ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.0.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: reth_trie::TrieInput,
    ) -> reth_errors::ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.0.state_root_from_nodes_with_updates(input)
    }
}

impl reth_storage_api::StorageRootProvider for StateProviderTraitObjWrapper {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.0.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        self.0.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageMultiProof> {
        self.0.storage_multiproof(address, slots, hashed_storage)
    }
}

impl reth_storage_api::StateProofProvider for StateProviderTraitObjWrapper {
    fn proof(
        &self,
        input: reth_trie::TrieInput,
        address: Address,
        slots: &[B256],
    ) -> reth_errors::ProviderResult<reth_trie::AccountProof> {
        self.0.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: reth_trie::TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<reth_trie::MultiProof> {
        self.0.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: reth_trie::TrieInput,
        target: reth_trie::HashedPostState,
        mode: reth_trie::ExecutionWitnessMode,
    ) -> reth_errors::ProviderResult<Vec<alloy_primitives::Bytes>> {
        self.0.witness(input, target, mode)
    }
}

impl reth_storage_api::AccountReader for StateProviderTraitObjWrapper {
    fn basic_account(
        &self,
        address: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Account>> {
        self.0.basic_account(address)
    }
}

impl reth_storage_api::BlockHashReader for StateProviderTraitObjWrapper {
    fn block_hash(
        &self,
        block_number: alloy_primitives::BlockNumber,
    ) -> reth_errors::ProviderResult<Option<B256>> {
        self.0.block_hash(block_number)
    }

    fn convert_block_hash(
        &self,
        hash_or_number: alloy_rpc_types_eth::BlockHashOrNumber,
    ) -> reth_errors::ProviderResult<Option<B256>> {
        self.0.convert_block_hash(hash_or_number)
    }

    fn canonical_hashes_range(
        &self,
        start: alloy_primitives::BlockNumber,
        end: alloy_primitives::BlockNumber,
    ) -> reth_errors::ProviderResult<Vec<B256>> {
        self.0.canonical_hashes_range(start, end)
    }
}

impl HashedPostStateProvider for StateProviderTraitObjWrapper {
    fn hashed_post_state(
        &self,
        bundle_state: &BundleState,
    ) -> ProviderResult<reth_trie::HashedPostState> {
        self.0.hashed_post_state(bundle_state)
    }
}

impl StateProvider for StateProviderTraitObjWrapper {
    fn storage(
        &self,
        account: Address,
        storage_key: alloy_primitives::StorageKey,
    ) -> reth_errors::ProviderResult<Option<alloy_primitives::StorageValue>> {
        self.0.storage(account, storage_key)
    }

    fn account_code(
        &self,
        addr: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.0.account_code(addr)
    }

    fn account_balance(&self, addr: &Address) -> reth_errors::ProviderResult<Option<U256>> {
        self.0.account_balance(addr)
    }

    fn account_nonce(&self, addr: &Address) -> reth_errors::ProviderResult<Option<u64>> {
        self.0.account_nonce(addr)
    }
}

impl BytecodeReader for StateProviderTraitObjWrapper {
    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.0.bytecode_by_hash(code_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Bytes, U256};
    use revm::{
        database::{CacheDB, EmptyDB},
        state::{
            bal::{AccountBal, Bal, BalWrites, BlockAccessIndex},
            AccountInfo,
        },
    };

    #[test]
    fn attach_bal_before_tx_serves_positioned_reads() {
        let covered = address!("0x0000000000000000000000000000000000000001");
        let uncovered = address!("0x0000000000000000000000000000000000000002");
        let written_slot = U256::from(1);
        let read_slot = U256::from(2);

        // pre-block state
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            covered,
            AccountInfo { balance: U256::from(7), nonce: 5, ..Default::default() },
        );
        db.insert_account_storage(covered, written_slot, U256::from(11)).unwrap();
        db.insert_account_storage(covered, read_slot, U256::from(99)).unwrap();
        db.insert_account_info(
            uncovered,
            AccountInfo { balance: U256::from(3), ..Default::default() },
        );

        // tx 1 writes the slot, tx 2 changes the balance
        let mut account = AccountBal::default();
        account.storage.storage.insert(
            written_slot,
            BalWrites::new(vec![(BlockAccessIndex::from_tx_index(1), U256::from(42))]),
        );
        account.account_info.balance =
            BalWrites::new(vec![(BlockAccessIndex::from_tx_index(2), U256::from(1000))]);
        let mut bal = Bal::default();
        bal.accounts.insert(covered, account);
        let bal = DecodedBal::new(Arc::new(bal), Bytes::new());

        let mut state = State::builder().with_database(db).build();

        // before tx 0, none of the block's writes are visible
        attach_bal_before_tx(&mut state, &bal, 0);
        assert_eq!(Database::storage(&mut state, covered, written_slot).unwrap(), U256::from(11));
        assert_eq!(Database::basic(&mut state, covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 2, the storage write of tx 1 is visible, the balance change of tx 2 is not
        attach_bal_before_tx(&mut state, &bal, 2);
        assert_eq!(Database::storage(&mut state, covered, written_slot).unwrap(), U256::from(42));
        assert_eq!(Database::basic(&mut state, covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 3, the balance change of tx 2 is visible
        attach_bal_before_tx(&mut state, &bal, 3);
        assert_eq!(
            Database::basic(&mut state, covered).unwrap().unwrap().balance,
            U256::from(1000)
        );

        // reads not covered by the BAL fall back to the underlying database
        assert_eq!(Database::storage(&mut state, covered, read_slot).unwrap(), U256::from(99));
        assert_eq!(Database::basic(&mut state, uncovered).unwrap().unwrap().balance, U256::from(3));
    }
}
