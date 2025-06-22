//! utilities for working with revm

use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rpc_types_eth::{
    state::{AccountOverride, StateOverride},
    BlockOverrides,
};
use revm::{
    context::BlockEnv,
    database::{CacheDB, State},
    state::{Account, AccountStatus, Bytecode, EvmStorageSlot},
    Database, DatabaseCommit,
};
use std::collections::{BTreeMap, HashMap};

use super::{EthApiError, EthResult, RpcInvalidTransactionError};

/// Calculates the caller gas allowance.
///
/// `allowance = (account.balance - tx.value) / tx.gas_price`
///
/// Returns an error if the caller has insufficient funds.
/// Caution: This assumes non-zero `env.gas_price`. Otherwise, zero allowance will be returned.
///
/// Note: this takes the mut [Database] trait because the loaded sender can be reused for the
/// following operation like `eth_call`.
pub fn caller_gas_allowance<DB, T>(db: &mut DB, env: &T) -> EthResult<u64>
where
    DB: Database,
    EthApiError: From<<DB as Database>::Error>,
    T: revm::context_interface::Transaction,
{
    // Get the caller account.
    let caller = db.basic(env.caller())?;
    // Get the caller balance.
    let balance = caller.map(|acc| acc.balance).unwrap_or_default();
    // Get transaction value.
    let value = env.value();
    // Subtract transferred value from the caller balance. Return error if the caller has
    // insufficient funds.
    let balance = balance
        .checked_sub(env.value())
        .ok_or_else(|| RpcInvalidTransactionError::InsufficientFunds { cost: value, balance })?;

    Ok(balance
        // Calculate the amount of gas the caller can afford with the specified gas price.
        .checked_div(U256::from(env.gas_price()))
        // This will be 0 if gas price is 0. It is fine, because we check it before.
        .unwrap_or_default()
        .saturating_to())
}

/// Helper trait implemented for databases that support overriding block hashes.
///
/// Used for applying [`BlockOverrides::block_hash`]
pub trait OverrideBlockHashes {
    /// Overrides the given block hashes.
    fn override_block_hashes(&mut self, block_hashes: BTreeMap<u64, B256>);
}

impl<DB> OverrideBlockHashes for CacheDB<DB> {
    fn override_block_hashes(&mut self, block_hashes: BTreeMap<u64, B256>) {
        self.cache
            .block_hashes
            .extend(block_hashes.into_iter().map(|(num, hash)| (U256::from(num), hash)))
    }
}

impl<DB> OverrideBlockHashes for State<DB> {
    fn override_block_hashes(&mut self, block_hashes: BTreeMap<u64, B256>) {
        self.block_hashes.extend(block_hashes);
    }
}

/// Applies the given block overrides to the env and updates overridden block hashes in the db.
pub fn apply_block_overrides(
    overrides: BlockOverrides,
    db: &mut impl OverrideBlockHashes,
    env: &mut BlockEnv,
) {
    let BlockOverrides {
        number,
        difficulty,
        time,
        gas_limit,
        coinbase,
        random,
        base_fee,
        block_hash,
    } = overrides;

    if let Some(block_hashes) = block_hash {
        // override block hashes
        db.override_block_hashes(block_hashes);
    }

    if let Some(number) = number {
        env.number = number.saturating_to();
    }
    if let Some(difficulty) = difficulty {
        env.difficulty = difficulty;
    }
    if let Some(time) = time {
        env.timestamp = U256::from(time);
    }
    if let Some(gas_limit) = gas_limit {
        env.gas_limit = gas_limit;
    }
    if let Some(coinbase) = coinbase {
        env.beneficiary = coinbase;
    }
    if let Some(random) = random {
        env.prevrandao = Some(random);
    }
    if let Some(base_fee) = base_fee {
        env.basefee = base_fee.saturating_to();
    }
}

/// Applies the given state overrides (a set of [`AccountOverride`]) to the [`CacheDB`].
pub fn apply_state_overrides<DB>(overrides: StateOverride, db: &mut DB) -> EthResult<()>
where
    DB: Database + DatabaseCommit,
    EthApiError: From<DB::Error>,
{
    for (account, account_overrides) in overrides {
        apply_account_override(account, account_overrides, db)?;
    }
    Ok(())
}

/// Applies a single [`AccountOverride`] to the [`CacheDB`].
fn apply_account_override<DB>(
    account: Address,
    account_override: AccountOverride,
    db: &mut DB,
) -> EthResult<()>
where
    DB: Database + DatabaseCommit,
    EthApiError: From<DB::Error>,
{
    let mut info = db.basic(account)?.unwrap_or_default();

    if let Some(nonce) = account_override.nonce {
        info.nonce = nonce;
    }
    if let Some(code) = account_override.code {
        // we need to set both the bytecode and the codehash
        info.code_hash = keccak256(&code);
        info.code = Some(
            Bytecode::new_raw_checked(code)
                .map_err(|err| EthApiError::InvalidBytecode(err.to_string()))?,
        );
    }
    if let Some(balance) = account_override.balance {
        info.balance = balance;
    }

    // Create a new account marked as touched
    let mut acc = revm::state::Account {
        info,
        status: AccountStatus::Touched,
        storage: HashMap::default(),
        transaction_id: 0,
    };

    let storage_diff = match (account_override.state, account_override.state_diff) {
        (Some(_), Some(_)) => return Err(EthApiError::BothStateAndStateDiffInOverride(account)),
        (None, None) => None,
        // If we need to override the entire state, we firstly mark account as destroyed to clear
        // its storage, and then we mark it is "NewlyCreated" to make sure that old storage won't be
        // used.
        (Some(state), None) => {
            // Destroy the account to ensure that its storage is cleared
            db.commit(HashMap::from_iter([(
                account,
                Account {
                    status: AccountStatus::SelfDestructed | AccountStatus::Touched,
                    ..Default::default()
                },
            )]));
            // Mark the account as created to ensure that old storage is not read
            acc.mark_created();
            Some(state)
        }
        (None, Some(state)) => Some(state),
    };

    if let Some(state) = storage_diff {
        for (slot, value) in state {
            acc.storage.insert(
                slot.into(),
                EvmStorageSlot {
                    // we use inverted value here to ensure that storage is treated as changed
                    original_value: (!value).into(),
                    present_value: value.into(),
                    is_cold: false,
                    transaction_id: 0,
                },
            );
        }
    }

    db.commit(HashMap::from_iter([(account, acc)]));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes};
    use reth_revm::db::EmptyDB;

    #[test]
    fn state_override_state() {
        let code = bytes!(
        "0x63d0e30db05f525f5f6004601c3473c02aaa39b223fe8d0a0e5c4f27ead9083c756cc25af15f5260205ff3"
    );
        let to = address!("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599");

        let mut db = State::builder().with_database(CacheDB::new(EmptyDB::new())).build();

        let acc_override = AccountOverride::default().with_code(code.clone());
        apply_account_override(to, acc_override, &mut db).unwrap();

        let account = db.basic(to).unwrap().unwrap();
        assert!(account.code.is_some());
        assert_eq!(account.code_hash, keccak256(&code));
    }

    #[test]
    fn state_override_cache_db() {
        let code = bytes!(
        "0x63d0e30db05f525f5f6004601c3473c02aaa39b223fe8d0a0e5c4f27ead9083c756cc25af15f5260205ff3"
    );
        let to = address!("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599");

        let mut db = CacheDB::new(EmptyDB::new());

        let acc_override = AccountOverride::default().with_code(code.clone());
        apply_account_override(to, acc_override, &mut db).unwrap();

        let account = db.basic(to).unwrap().unwrap();
        assert!(account.code.is_some());
        assert_eq!(account.code_hash, keccak256(&code));
    }
}
