//! EVM database types used by the RPC state cache.

use alloy_eip7928::{bal::DecodedBal, BlockAccessIndex};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::StateProviderBox;
use revm::{database::State, state::bal::Bal as RevmBal, Database};
use std::sync::Arc;

/// Helper alias type for the state's [`State`]
pub type StateCacheDb = State<StateProviderDatabase<StateProviderBox>>;

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
