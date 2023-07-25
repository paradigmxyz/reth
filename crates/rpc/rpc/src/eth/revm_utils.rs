//! utilities for working with revm

use crate::eth::error::{EthApiError, EthResult, RpcInvalidTransactionError};
use reth_primitives::{
    AccessList, Address, TransactionSigned, TransactionSignedEcRecovered, TxHash, H256, U256,
};
use reth_revm::env::{fill_tx_env, fill_tx_env_with_recovered};
use reth_rpc_types::{
    state::{AccountOverride, StateOverride},
    BlockOverrides, CallRequest,
};
use revm::{
    db::{CacheDB, EmptyDB},
    precompile::{Precompiles, SpecId as PrecompilesSpecId},
    primitives::{BlockEnv, CfgEnv, Env, ResultAndState, SpecId, TransactTo, TxEnv},
    Database, Inspector,
};
use revm_primitives::{
    db::{DatabaseCommit, DatabaseRef},
    Bytecode, ExecutionResult,
};
use tracing::trace;

/// Helper type that bundles various overrides for EVM Execution.
///
/// By `Default`, no overrides are included.
#[derive(Debug, Clone, Default)]
pub struct EvmOverrides {
    /// Applies overrides to the state before execution.
    pub state: Option<StateOverride>,
    /// Applies overrides to the block before execution.
    ///
    /// This is a `Box` because less common and only available in debug trace endpoints.
    pub block: Option<Box<BlockOverrides>>,
}

impl EvmOverrides {
    /// Creates a new instance with the given overrides
    pub fn new(state: Option<StateOverride>, block: Option<Box<BlockOverrides>>) -> Self {
        Self { state, block }
    }

    /// Creates a new instance with the given state overrides.
    pub fn state(state: Option<StateOverride>) -> Self {
        Self { state, block: None }
    }

    /// Returns `true` if the overrides contain state overrides.
    pub fn has_state(&self) -> bool {
        self.state.is_some()
    }
}

impl From<Option<StateOverride>> for EvmOverrides {
    fn from(state: Option<StateOverride>) -> Self {
        Self::state(state)
    }
}

/// Helper type to work with different transaction types when configuring the EVM env.
///
/// This makes it easier to handle errors.
pub(crate) trait FillableTransaction {
    /// Returns the hash of the transaction.
    fn hash(&self) -> TxHash;

    /// Fill the transaction environment with the given transaction.
    fn try_fill_tx_env(&self, tx_env: &mut TxEnv) -> EthResult<()>;
}

impl FillableTransaction for TransactionSignedEcRecovered {
    fn hash(&self) -> TxHash {
        self.hash
    }

    fn try_fill_tx_env(&self, tx_env: &mut TxEnv) -> EthResult<()> {
        fill_tx_env_with_recovered(tx_env, self);
        Ok(())
    }
}
impl FillableTransaction for TransactionSigned {
    fn hash(&self) -> TxHash {
        self.hash
    }

    fn try_fill_tx_env(&self, tx_env: &mut TxEnv) -> EthResult<()> {
        let signer =
            self.recover_signer().ok_or_else(|| EthApiError::InvalidTransactionSignature)?;
        fill_tx_env(tx_env, self, signer);
        Ok(())
    }
}

/// Returns the addresses of the precompiles corresponding to the SpecId.
pub(crate) fn get_precompiles(spec_id: &SpecId) -> Vec<reth_primitives::H160> {
    let spec = match spec_id {
        SpecId::FRONTIER | SpecId::FRONTIER_THAWING => return vec![],
        SpecId::HOMESTEAD | SpecId::DAO_FORK | SpecId::TANGERINE | SpecId::SPURIOUS_DRAGON => {
            PrecompilesSpecId::HOMESTEAD
        }
        SpecId::BYZANTIUM | SpecId::CONSTANTINOPLE | SpecId::PETERSBURG => {
            PrecompilesSpecId::BYZANTIUM
        }
        SpecId::ISTANBUL | SpecId::MUIR_GLACIER => PrecompilesSpecId::ISTANBUL,
        SpecId::BERLIN |
        SpecId::LONDON |
        SpecId::ARROW_GLACIER |
        SpecId::GRAY_GLACIER |
        SpecId::MERGE |
        SpecId::SHANGHAI |
        SpecId::CANCUN => PrecompilesSpecId::BERLIN,
        SpecId::LATEST => PrecompilesSpecId::LATEST,
    };
    Precompiles::new(spec).addresses().into_iter().map(Address::from).collect()
}

/// Executes the [Env] against the given [Database] without committing state changes.
pub(crate) fn transact<DB>(db: DB, env: Env) -> EthResult<(ResultAndState, Env)>
where
    DB: Database,
    <DB as Database>::Error: Into<EthApiError>,
{
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    let res = evm.transact()?;
    Ok((res, evm.env))
}

/// Executes the [Env] against the given [Database] without committing state changes.
pub(crate) fn inspect<DB, I>(db: DB, env: Env, inspector: I) -> EthResult<(ResultAndState, Env)>
where
    DB: Database,
    <DB as Database>::Error: Into<EthApiError>,
    I: Inspector<DB>,
{
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    let res = evm.inspect(inspector)?;
    Ok((res, evm.env))
}

/// Same as [inspect] but also returns the database again.
///
/// Even though [Database] is also implemented on `&mut`
/// this is still useful if there are certain trait bounds on the Inspector's database generic type
pub(crate) fn inspect_and_return_db<DB, I>(
    db: DB,
    env: Env,
    inspector: I,
) -> EthResult<(ResultAndState, Env, DB)>
where
    DB: Database,
    <DB as Database>::Error: Into<EthApiError>,
    I: Inspector<DB>,
{
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    let res = evm.inspect(inspector)?;
    let db = evm.take_db();
    Ok((res, evm.env, db))
}

/// Replays all the transactions until the target transaction is found.
///
/// All transactions before the target transaction are executed and their changes are written to the
/// _runtime_ db ([CacheDB]).
///
/// Note: This assumes the target transaction is in the given iterator.
pub(crate) fn replay_transactions_until<DB, I, Tx>(
    db: &mut CacheDB<DB>,
    cfg: CfgEnv,
    block_env: BlockEnv,
    transactions: I,
    target_tx_hash: H256,
) -> EthResult<()>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
    I: IntoIterator<Item = Tx>,
    Tx: FillableTransaction,
{
    let env = Env { cfg, block: block_env, tx: TxEnv::default() };
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    for tx in transactions.into_iter() {
        if tx.hash() == target_tx_hash {
            // reached the target transaction
            break
        }

        tx.try_fill_tx_env(&mut evm.env.tx)?;
        let res = evm.transact()?;
        evm.db.as_mut().expect("is set").commit(res.state)
    }
    Ok(())
}

/// Prepares the [Env] for execution.
///
/// Does not commit any changes to the underlying database.
pub(crate) fn prepare_call_env<DB>(
    mut cfg: CfgEnv,
    block: BlockEnv,
    request: CallRequest,
    gas_limit: u64,
    db: &mut CacheDB<DB>,
    overrides: EvmOverrides,
) -> EthResult<Env>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
{
    // we want to disable this in eth_call, since this is common practice used by other node
    // impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
    cfg.disable_block_gas_limit = true;

    // Disabled because eth_call is sometimes used with eoa senders
    // See <https://github.com/paradigmxyz/reth/issues/1959>
    cfg.disable_eip3607 = true;

    // The basefee should be ignored for eth_call
    // See:
    // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
    cfg.disable_base_fee = true;

    let request_gas = request.gas;

    let mut env = build_call_evm_env(cfg, block, request)?;

    // apply state overrides
    if let Some(state_overrides) = overrides.state {
        apply_state_overrides(state_overrides, db)?;
    }

    // apply block overrides
    if let Some(block_overrides) = overrides.block {
        apply_block_overrides(*block_overrides, &mut env.block);
    }

    if request_gas.is_none() {
        // No gas limit was provided in the request, so we need to cap the transaction gas limit
        if env.tx.gas_price > U256::ZERO {
            // If gas price is specified, cap transaction gas limit with caller allowance
            trace!(target: "rpc::eth::call", ?env, "Applying gas limit cap with caller allowance");
            cap_tx_gas_limit_with_caller_allowance(db, &mut env.tx)?;
        } else {
            // If no gas price is specified, use maximum allowed gas limit. The reason for this is
            // that both Erigon and Geth use pre-configured gas cap even if it's possible
            // to derive the gas limit from the block:
            // <https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/cmd/rpcdaemon/commands/trace_adhoc.go#L956
            // https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/eth/ethconfig/config.go#L94>
            trace!(target: "rpc::eth::call", ?env, "Applying gas limit cap as the maximum gas limit");
            env.tx.gas_limit = gas_limit;
        }
    }

    Ok(env)
}

/// Creates a new [Env] to be used for executing the [CallRequest] in `eth_call`.
///
/// Note: this does _not_ access the Database to check the sender.
pub(crate) fn build_call_evm_env(
    cfg: CfgEnv,
    block: BlockEnv,
    request: CallRequest,
) -> EthResult<Env> {
    let tx = create_txn_env(&block, request)?;
    Ok(Env { cfg, block, tx })
}

/// Configures a new [TxEnv]  for the [CallRequest]
///
/// All [TxEnv] fields are derived from the given [CallRequest], if fields are `None`, they fall
/// back to the [BlockEnv]'s settings.
pub(crate) fn create_txn_env(block_env: &BlockEnv, request: CallRequest) -> EthResult<TxEnv> {
    let CallRequest {
        from,
        to,
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        gas,
        value,
        input,
        nonce,
        access_list,
        chain_id,
        ..
    } = request;

    let CallFees { max_priority_fee_per_gas, gas_price } = CallFees::ensure_fees(
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        block_env.basefee,
    )?;

    let gas_limit = gas.unwrap_or(block_env.gas_limit.min(U256::from(u64::MAX)));

    let env = TxEnv {
        gas_limit: gas_limit.try_into().map_err(|_| RpcInvalidTransactionError::GasUintOverflow)?,
        nonce: nonce
            .map(|n| n.try_into().map_err(|_| RpcInvalidTransactionError::NonceTooHigh))
            .transpose()?,
        caller: from.unwrap_or_default(),
        gas_price,
        gas_priority_fee: max_priority_fee_per_gas,
        transact_to: to.map(TransactTo::Call).unwrap_or_else(TransactTo::create),
        value: value.unwrap_or_default(),
        data: input.try_into_unique_input()?.map(|data| data.0).unwrap_or_default(),
        chain_id: chain_id.map(|c| c.as_u64()),
        access_list: access_list.map(AccessList::flattened).unwrap_or_default(),
    };

    Ok(env)
}

/// Caps the configured [TxEnv] `gas_limit` with the allowance of the caller.
pub(crate) fn cap_tx_gas_limit_with_caller_allowance<DB>(db: DB, env: &mut TxEnv) -> EthResult<()>
where
    DB: Database,
    EthApiError: From<<DB as Database>::Error>,
{
    if let Ok(gas_limit) = caller_gas_allowance(db, env)?.try_into() {
        env.gas_limit = gas_limit;
    }

    Ok(())
}

/// Calculates the caller gas allowance.
///
/// `allowance = (account.balance - tx.value) / tx.gas_price`
///
/// Returns an error if the caller has insufficient funds.
/// Caution: This assumes non-zero `env.gas_price`. Otherwise, zero allowance will be returned.
pub(crate) fn caller_gas_allowance<DB>(mut db: DB, env: &TxEnv) -> EthResult<U256>
where
    DB: Database,
    EthApiError: From<<DB as Database>::Error>,
{
    Ok(db
        // Get the caller account.
        .basic(env.caller)?
        // Get the caller balance.
        .map(|acc| acc.balance)
        .unwrap_or_default()
        // Subtract transferred value from the caller balance.
        .checked_sub(env.value)
        // Return error if the caller has insufficient funds.
        .ok_or_else(|| RpcInvalidTransactionError::InsufficientFunds)?
        // Calculate the amount of gas the caller can afford with the specified gas price.
        .checked_div(env.gas_price)
        // This will be 0 if gas price is 0. It is fine, because we check it before.
        .unwrap_or_default())
}

/// Helper type for representing the fees of a [CallRequest]
pub(crate) struct CallFees {
    /// EIP-1559 priority fee
    max_priority_fee_per_gas: Option<U256>,
    /// Unified gas price setting
    ///
    /// Will be the configured `basefee` if unset in the request
    ///
    /// `gasPrice` for legacy,
    /// `maxFeePerGas` for EIP-1559
    gas_price: U256,
}

// === impl CallFees ===

impl CallFees {
    /// Ensures the fields of a [CallRequest] are not conflicting.
    ///
    /// If no `gasPrice` or `maxFeePerGas` is set, then the `gas_price` in the returned `gas_price`
    /// will be `0`. See: <https://github.com/ethereum/go-ethereum/blob/2754b197c935ee63101cbbca2752338246384fec/internal/ethapi/transaction_args.go#L242-L255>
    fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        base_fee: U256,
    ) -> EthResult<CallFees> {
        match (call_gas_price, call_max_fee, call_priority_fee) {
            (gas_price, None, None) => {
                // either legacy transaction or no fee fields are specified
                // when no fields are specified, set gas price to zero
                let gas_price = gas_price.unwrap_or(U256::ZERO);
                Ok(CallFees { gas_price, max_priority_fee_per_gas: None })
            }
            (None, max_fee_per_gas, max_priority_fee_per_gas) => {
                // request for eip-1559 transaction
                let max_fee = max_fee_per_gas.unwrap_or(base_fee);

                if let Some(max_priority) = max_priority_fee_per_gas {
                    if max_priority > max_fee {
                        // Fail early
                        return Err(
                            // `max_priority_fee_per_gas` is greater than the `max_fee_per_gas`
                            RpcInvalidTransactionError::TipAboveFeeCap.into(),
                        )
                    }
                }
                Ok(CallFees { gas_price: max_fee, max_priority_fee_per_gas })
            }
            _ => Err(EthApiError::ConflictingFeeFieldsInRequest),
        }
    }
}

/// Applies the given block overrides to the env
fn apply_block_overrides(overrides: BlockOverrides, env: &mut BlockEnv) {
    let BlockOverrides { number, difficulty, time, gas_limit, coinbase, random, base_fee } =
        overrides;

    if let Some(number) = number {
        env.number = number;
    }
    if let Some(difficulty) = difficulty {
        env.difficulty = difficulty;
    }
    if let Some(time) = time {
        env.timestamp = U256::from(time.as_u64());
    }
    if let Some(gas_limit) = gas_limit {
        env.gas_limit = U256::from(gas_limit.as_u64());
    }
    if let Some(coinbase) = coinbase {
        env.coinbase = coinbase;
    }
    if let Some(random) = random {
        env.prevrandao = Some(random);
    }
    if let Some(base_fee) = base_fee {
        env.basefee = base_fee;
    }
}

/// Applies the given state overrides (a set of [AccountOverride]) to the [CacheDB].
fn apply_state_overrides<DB>(overrides: StateOverride, db: &mut CacheDB<DB>) -> EthResult<()>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
{
    for (account, account_overrides) in overrides {
        apply_account_override(account, account_overrides, db)?;
    }
    Ok(())
}

/// Applies a single [AccountOverride] to the [CacheDB].
fn apply_account_override<DB>(
    account: Address,
    account_override: AccountOverride,
    db: &mut CacheDB<DB>,
) -> EthResult<()>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
{
    // we need to fetch the account via the `DatabaseRef` to not update the state of the account,
    // which is modified via `Database::basic`
    let mut account_info = DatabaseRef::basic(db, account)?.unwrap_or_default();

    if let Some(nonce) = account_override.nonce {
        account_info.nonce = nonce.as_u64();
    }
    if let Some(code) = account_override.code {
        account_info.code = Some(Bytecode::new_raw(code.0));
    }
    if let Some(balance) = account_override.balance {
        account_info.balance = balance;
    }

    db.insert_account_info(account, account_info);

    // We ensure that not both state and state_diff are set.
    // If state is set, we must mark the account as "NewlyCreated", so that the old storage
    // isn't read from
    match (account_override.state, account_override.state_diff) {
        (Some(_), Some(_)) => return Err(EthApiError::BothStateAndStateDiffInOverride(account)),
        (None, None) => {
            // nothing to do
        }
        (Some(new_account_state), None) => {
            db.replace_account_storage(
                account,
                new_account_state
                    .into_iter()
                    .map(|(slot, value)| {
                        (U256::from_be_bytes(slot.0), U256::from_be_bytes(value.0))
                    })
                    .collect(),
            )?;
        }
        (None, Some(account_state_diff)) => {
            for (slot, value) in account_state_diff {
                db.insert_account_storage(
                    account,
                    U256::from_be_bytes(slot.0),
                    U256::from_be_bytes(value.0),
                )?;
            }
        }
    };

    Ok(())
}

/// This clones and transforms the given [CacheDB] with an arbitrary [DatabaseRef] into a new
/// [CacheDB] with [EmptyDB] as the database type
#[inline]
pub(crate) fn clone_into_empty_db<DB>(db: &CacheDB<DB>) -> CacheDB<EmptyDB>
where
    DB: DatabaseRef,
{
    CacheDB {
        accounts: db.accounts.clone(),
        contracts: db.contracts.clone(),
        logs: db.logs.clone(),
        block_hashes: db.block_hashes.clone(),
        db: Default::default(),
    }
}

/// Helper to get the output data from a result
///
/// TODO: Can be phased out when <https://github.com/bluealloy/revm/pull/509> is released
#[inline]
pub(crate) fn result_output(res: &ExecutionResult) -> Option<bytes::Bytes> {
    match res {
        ExecutionResult::Success { output, .. } => Some(output.clone().into_data()),
        ExecutionResult::Revert { output, .. } => Some(output.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_0_fallback() {
        let CallFees { gas_price, .. } =
            CallFees::ensure_fees(None, None, None, U256::from(99)).unwrap();
        assert_eq!(gas_price, U256::ZERO);
    }
}
