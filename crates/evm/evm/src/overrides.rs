//! RPC override helpers for EVM environments.

use alloc::collections::BTreeMap;
use alloy_primitives::{keccak256, map::HashMap, Address, B256, U256};
use alloy_rpc_types_eth::{
    state::{AccountOverride, StateOverride},
    BlockOverrides,
};
use reth_execution_types::{
    Account, Bytecode, EvmAccountStatus, EvmStorageSlot, State, TransactionId,
};
use revm::{
    bytecode::BytecodeDecodeError, context::BlockEnv,
    context_interface::block::BlobExcessGasAndPrice,
};

use crate::{Database, DatabaseCommit};

/// Errors that can occur when applying state overrides.
#[derive(Debug, thiserror::Error)]
pub enum StateOverrideError<E> {
    /// Invalid bytecode provided in override.
    #[error(transparent)]
    InvalidBytecode(#[from] BytecodeDecodeError),
    /// Both state and state_diff were provided for an account.
    #[error("Both 'state' and 'stateDiff' fields are set for account {0}")]
    BothStateAndStateDiff(Address),
    /// Database error occurred.
    #[error(transparent)]
    Database(E),
}

/// Helper trait implemented for databases that support overriding block hashes.
pub trait OverrideBlockHashes {
    /// Overrides the given block hashes.
    fn override_block_hashes(&mut self, block_hashes: BTreeMap<u64, B256>);

    /// Applies block overrides to the env and updates overridden block hashes.
    fn apply_block_overrides(&mut self, overrides: BlockOverrides, env: &mut BlockEnv)
    where
        Self: Sized,
    {
        apply_block_overrides(overrides, self, env);
    }
}

impl<DB> OverrideBlockHashes for State<DB> {
    fn override_block_hashes(&mut self, block_hashes: BTreeMap<u64, B256>) {
        self.block_hashes.extend(block_hashes);
    }
}

/// Applies block overrides to the env and updates overridden block hashes in the db.
pub fn apply_block_overrides<DB>(overrides: BlockOverrides, db: &mut DB, env: &mut BlockEnv)
where
    DB: OverrideBlockHashes,
{
    #[allow(clippy::needless_update)]
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
    } = BlockOverrides { ..overrides };

    if let Some(block_hashes) = block_hash {
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
    if let Some(blob_base_fee) = blob_base_fee {
        let excess_blob_gas =
            env.blob_excess_gas_and_price.map(|blob| blob.excess_blob_gas).unwrap_or_default();
        env.blob_excess_gas_and_price = Some(BlobExcessGasAndPrice {
            excess_blob_gas,
            blob_gasprice: blob_base_fee.saturating_to(),
        });
    }
}

/// Applies state overrides to the database.
pub fn apply_state_overrides<DB>(
    overrides: StateOverride,
    db: &mut DB,
) -> Result<(), StateOverrideError<DB::Error>>
where
    DB: Database + DatabaseCommit,
{
    for (account, account_overrides) in overrides {
        apply_account_override(account, account_overrides, db)?;
    }
    Ok(())
}

fn apply_account_override<DB>(
    account: Address,
    account_override: AccountOverride,
    db: &mut DB,
) -> Result<(), StateOverrideError<DB::Error>>
where
    DB: Database + DatabaseCommit,
{
    let mut info = db.basic(account).map_err(StateOverrideError::Database)?.unwrap_or_default();

    if let Some(nonce) = account_override.nonce {
        info.nonce = nonce;
    }
    if let Some(code) = account_override.code {
        info.code_hash = keccak256(&code);
        info.code = Some(Bytecode::new_raw_checked(code)?);
    }
    if let Some(balance) = account_override.balance {
        info.balance = balance;
    }

    let mut acc = Account::from(info);
    acc.status = EvmAccountStatus::Touched;

    let storage_diff = match (account_override.state, account_override.state_diff) {
        (Some(_), Some(_)) => return Err(StateOverrideError::BothStateAndStateDiff(account)),
        (None, None) => None,
        (Some(state), None) => {
            let mut destroyed = Account::default();
            destroyed.status = EvmAccountStatus::SelfDestructed | EvmAccountStatus::Touched;
            db.commit(HashMap::from_iter([(account, destroyed)]));
            acc.mark_created();
            Some(state)
        }
        (None, Some(state)) => {
            if acc.info.is_empty() && !state.is_empty() {
                acc.mark_created();
            }
            Some(state)
        }
    };

    if let Some(state) = storage_diff {
        for (slot, value) in state {
            acc.storage.insert(
                slot.into(),
                EvmStorageSlot {
                    original_value: (!value).into(),
                    present_value: value.into(),
                    is_cold: false,
                    transaction_id: TransactionId::ZERO,
                },
            );
        }
    }

    db.commit(HashMap::from_iter([(account, acc)]));

    Ok(())
}
