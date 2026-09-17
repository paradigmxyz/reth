//! Helper types to workaround 'higher-ranked lifetime error'
//! <https://github.com/rust-lang/rust/issues/100013> in default implementation of
//! `reth_rpc_eth_api::helpers::Call`.

use crate::error::StateOverrideError;
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides};
use evm2::{
    bytecode::Bytecode,
    evm::{CacheDB, Db, DynDatabase},
};
use reth_evm::{database::StateProviderDatabase, EvmEnv};
use reth_storage_api::StateProviderBox;
use alloy_eip7928::{bal::DecodedBal, BlockAccessIndex};
use evm2::evm::Bal as EvmBal;
use std::sync::Arc;

/// Helper alias type for cached state access.
pub type StateCacheDb = CacheDB<Db<StateProviderDatabase<StateProviderBox>>>;

/// Applies RPC block overrides to an evm2 environment and state cache.
pub fn apply_block_overrides<DB>(
    overrides: BlockOverrides,
    db: &mut CacheDB<DB>,
    evm_env: &mut impl EvmEnv,
) {
    let BlockOverrides {
        number,
        difficulty,
        time,
        gas_limit,
        coinbase,
        random,
        base_fee,
        blob_base_fee,
        block_hash,
        ..
    } = overrides;

    if let Some(block_hashes) = block_hash {
        for (number, hash) in block_hashes {
            db.insert_block_hash(&U256::from(number), &hash);
        }
    }

    let block = evm_env.block_env_mut();
    if let Some(number) = number {
        block.number = number;
    }
    if let Some(difficulty) = difficulty {
        block.difficulty = difficulty;
    }
    if let Some(time) = time {
        block.timestamp = U256::from(time);
    }
    if let Some(gas_limit) = gas_limit {
        block.gas_limit = U256::from(gas_limit);
    }
    if let Some(coinbase) = coinbase {
        block.beneficiary = coinbase;
    }
    if let Some(random) = random {
        block.prevrandao = U256::from_be_slice(random.as_slice());
    }
    if let Some(base_fee) = base_fee {
        block.basefee = U256::from(base_fee);
    }
    if let Some(blob_base_fee) = blob_base_fee {
        block.blob_basefee = U256::from(blob_base_fee);
    }
}

/// Applies RPC state overrides to an evm2 state cache.
pub fn apply_state_overrides<DB: DynDatabase>(
    overrides: StateOverride,
    db: &mut CacheDB<DB>,
) -> Result<(), StateOverrideError<evm2::AnyError>> {
    for (address, account_override) in overrides {
        let mut account = db
            .get_account(&address)
            .map_err(|code| StateOverrideError::Database(db.error(code)))?
            .unwrap_or_default();

        if let Some(nonce) = account_override.nonce {
            account.nonce = nonce;
        }
        if let Some(code) = account_override.code {
            let code = Bytecode::new_raw_checked(code)?;
            account.code_hash = code.hash_slow();
            account.code = Some(code);
        }
        if let Some(balance) = account_override.balance {
            account.balance = balance;
        }

        let storage = match (account_override.state, account_override.state_diff) {
            (Some(_), Some(_)) => return Err(StateOverrideError::BothStateAndStateDiff(address)),
            (Some(state), None) => {
                db.cache.storage.entry(address).or_default().wipe();
                Some(state)
            }
            (None, Some(state)) => Some(state),
            (None, None) => None,
        };

        db.insert_account_info(&address, account);
        if let Some(storage) = storage {
            for (key, value) in storage {
                db.insert_account_storage(
                    &address,
                    &U256::from_be_bytes(key.0),
                    &U256::from_be_bytes(value.0),
                );
            }
        }
    }

    Ok(())
}

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
pub fn attach_bal_before_tx<DB: DynDatabase>(
    db: &mut CacheDB<DB>,
    bal: &DecodedBal<Arc<EvmBal>>,
    tx_index: usize,
) {
    db.bal_context.set_bal(bal.as_bal().clone());
    db.bal_context.set_allow_db_fallback(true);
    db.bal_context.set_bal_index(BlockAccessIndex::from_tx_index(tx_index as u64));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Bytes, U256};
    use evm2::evm::{AccountBal, Bal, BalChanges, AccountInfo, EmptyDB};
    use alloy_eip7928::{StorageChange, BalanceChange};

    #[test]
    fn attach_bal_before_tx_serves_positioned_reads() {
        let covered = address!("0x0000000000000000000000000000000000000001");
        let uncovered = address!("0x0000000000000000000000000000000000000002");
        let written_slot = U256::from(1);
        let read_slot = U256::from(2);

        // pre-block state
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            &covered,
            AccountInfo { balance: U256::from(7), nonce: 5, ..Default::default() },
        );
        db.insert_account_storage(&covered, &written_slot, &U256::from(11));
        db.insert_account_storage(&covered, &read_slot, &U256::from(99));
        db.insert_account_info(
            &uncovered,
            AccountInfo { balance: U256::from(3), ..Default::default() },
        );

        // tx 1 writes the slot, tx 2 changes the balance
        let mut account = AccountBal::default();
        account.storage.storage.insert(
            written_slot,
            BalChanges::new(vec![StorageChange::new(BlockAccessIndex::from_tx_index(1), U256::from(42))]),
        );
        account.account_info.balance =
            BalChanges::new(vec![BalanceChange::new(BlockAccessIndex::from_tx_index(2), U256::from(1000))]);
        let mut bal = Bal::default();
        bal.accounts.insert(covered, account);
        let bal = DecodedBal::new(Arc::new(bal), Bytes::new());

        let mut state = CacheDB::new(db);

        // before tx 0, none of the block's writes are visible
        attach_bal_before_tx(&mut state, &bal, 0);
        assert_eq!(state.get_storage(&covered, &written_slot).unwrap(), U256::from(11));
        assert_eq!(state.get_account(&covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 2, the storage write of tx 1 is visible, the balance change of tx 2 is not
        attach_bal_before_tx(&mut state, &bal, 2);
        assert_eq!(state.get_storage(&covered, &written_slot).unwrap(), U256::from(42));
        assert_eq!(state.get_account(&covered).unwrap().unwrap().balance, U256::from(7));

        // before tx 3, the balance change of tx 2 is visible
        attach_bal_before_tx(&mut state, &bal, 3);
        assert_eq!(
            state.get_account(&covered).unwrap().unwrap().balance,
            U256::from(1000)
        );

        // reads not covered by the BAL fall back to the underlying database
        assert_eq!(state.get_storage(&covered, &read_slot).unwrap(), U256::from(99));
        assert_eq!(state.get_account(&uncovered).unwrap().unwrap().balance, U256::from(3));
    }
}
