//! RPC conversion helpers for EVM environments.

use crate::Database;
use alloy_primitives::U256;
use revm::context_interface::Transaction;

pub use alloy_evm::rpc::{CallFees, CallFeesError, EthTxEnvError, TryIntoTxEnv};

/// Insufficient caller funds for an RPC call.
#[derive(Debug, thiserror::Error)]
#[error("insufficient funds: cost {cost} > balance {balance}")]
pub struct InsufficientFundsError {
    /// Transaction cost.
    pub cost: U256,
    /// Account balance.
    pub balance: U256,
}

/// Error type for RPC call utilities.
#[derive(Debug, thiserror::Error)]
pub enum CallError<E> {
    /// Database error.
    #[error(transparent)]
    Database(E),
    /// Insufficient caller funds.
    #[error(transparent)]
    InsufficientFunds(#[from] InsufficientFundsError),
}

/// Calculates the caller gas allowance.
///
/// `allowance = (account.balance - tx.value) / tx.gas_price`
pub fn caller_gas_allowance<DB, T>(db: &mut DB, env: &T) -> Result<u64, CallError<DB::Error>>
where
    DB: Database,
    T: Transaction,
{
    let caller = db.basic(env.caller()).map_err(CallError::Database)?;
    let balance = caller.map(|acc| acc.balance).unwrap_or_default();
    let value = env.value();
    let balance =
        balance.checked_sub(value).ok_or(InsufficientFundsError { cost: value, balance })?;

    Ok(balance.checked_div(U256::from(env.gas_price())).unwrap_or_default().saturating_to())
}
