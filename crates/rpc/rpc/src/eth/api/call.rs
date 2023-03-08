//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

#![allow(unused)] // TODO rm later

use crate::{
    eth::error::{EthApiError, EthResult, InvalidTransactionError, RevertError},
    EthApi,
};
use ethers_core::utils::get_contract_address;
use reth_primitives::{
    AccessList, AccessListItem, AccessListWithGasUsed, Address, BlockId, BlockNumberOrTag, Bytes,
    TransactionKind, H256, U128, U256,
};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProvider, StateProviderFactory};
use reth_revm::{
    access_list::AccessListInspector,
    database::{State, SubState},
};
use reth_rpc_types::{
    state::{AccountOverride, StateOverride},
    CallRequest,
};
use revm::{
    db::{CacheDB, DatabaseRef},
    precompile::{Precompiles, SpecId as PrecompilesSpecId},
    primitives::{
        ruint::Uint, BlockEnv, Bytecode, CfgEnv, Env, ExecutionResult, Halt, ResultAndState,
        SpecId, TransactTo, TxEnv,
    },
    Database, Inspector,
};

// Gas per transaction not creating a contract.
const MIN_TRANSACTION_GAS: u64 = 21_000u64;
const MIN_CREATE_GAS: u64 = 53_000u64;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Returns the revm evm env for the requested [BlockId]
    ///
    /// If the [BlockId] this will return the [BlockId::Hash] of the block the env was configured
    /// for.
    async fn evm_env_at(&self, at: BlockId) -> EthResult<(CfgEnv, BlockEnv, BlockId)> {
        // TODO handle Pending state's env
        match at {
            BlockId::Number(BlockNumberOrTag::Pending) => {
                // This should perhaps use the latest env settings and update block specific
                // settings like basefee/number
                unimplemented!("support pending state env")
            }
            hash_or_num => {
                let block_hash = self
                    .client()
                    .block_hash_for_id(hash_or_num)?
                    .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
                let (cfg, env) = self.cache().get_evm_env(block_hash).await?;
                Ok((cfg, env, block_hash.into()))
            }
        }
    }

    /// Executes the call request at the given [BlockId]
    pub(crate) async fn call_at(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
    ) -> EthResult<(ResultAndState, Env)> {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;
        let state = self.state_at_block_id(at)?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        self.call_with(cfg, block_env, request, state, state_overrides)
    }

    /// Executes the call request using the given environment against the state provider
    ///
    /// Does not commit any changes to the database
    fn call_with<S>(
        &self,
        mut cfg: CfgEnv,
        block: BlockEnv,
        mut request: CallRequest,
        state: S,
        state_overrides: Option<StateOverride>,
    ) -> EthResult<(ResultAndState, Env)>
    where
        S: StateProvider,
    {
        // we want to disable this in eth_call, since this is common practice used by other node
        // impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
        cfg.disable_block_gas_limit = true;

        let mut env = build_call_evm_env(cfg, block, request)?;
        let mut db = SubState::new(State::new(state));

        // apply state overrides
        if let Some(state_overrides) = state_overrides {
            apply_state_overrides(state_overrides, &mut db)?;
        }

        transact(&mut db, env)
    }

    /// Estimate gas needed for execution of the `request` at the [BlockId].
    pub(crate) async fn estimate_gas_at(
        &self,
        request: CallRequest,
        at: BlockId,
    ) -> EthResult<U256> {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;
        let state = self.state_at_block_id(at)?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        self.estimate_gas_with(cfg, block_env, request, state)
    }

    /// Estimates the gas usage of the `request` with the state.
    ///
    /// This will execute the [CallRequest] and find the best gas limit via binary search
    fn estimate_gas_with<S>(
        &self,
        cfg: CfgEnv,
        block: BlockEnv,
        mut request: CallRequest,
        state: S,
    ) -> EthResult<U256>
    where
        S: StateProvider,
    {
        // keep a copy of gas related request values
        let request_gas = request.gas;
        let request_gas_price = request.gas_price;
        let env_gas_limit = block.gas_limit;

        // get the highest possible gas limit, either the request's set value or the currently
        // configured gas limit
        let mut highest_gas_limit = request.gas.unwrap_or(block.gas_limit);

        // Configure the evm env
        let mut env = build_call_evm_env(cfg, block, request)?;
        let mut db = SubState::new(State::new(state));

        // if the request is a simple transfer we can optimize
        if env.tx.data.is_empty() {
            if let TransactTo::Call(to) = env.tx.transact_to {
                if let Ok(code) = db.db.state().account_code(to) {
                    let no_code_callee = code.map(|code| code.is_empty()).unwrap_or(true);
                    if no_code_callee {
                        // simple transfer, check if caller has sufficient funds
                        let mut available_funds =
                            db.basic(env.tx.caller)?.map(|acc| acc.balance).unwrap_or_default();
                        if env.tx.value > available_funds {
                            return Err(InvalidTransactionError::InsufficientFundsForTransfer.into())
                        }
                        return Ok(U256::from(MIN_TRANSACTION_GAS))
                    }
                }
            }
        }

        // check funds of the sender
        let gas_price = env.tx.gas_price;
        if gas_price > U256::ZERO {
            let mut available_funds =
                db.basic(env.tx.caller)?.map(|acc| acc.balance).unwrap_or_default();
            if env.tx.value > available_funds {
                return Err(InvalidTransactionError::InsufficientFunds.into())
            }
            // subtract transferred value from available funds
            // SAFETY: value < available_funds, checked above
            available_funds -= env.tx.value;
            // amount of gas the sender can afford with the `gas_price`
            // SAFETY: gas_price not zero
            let allowance = available_funds.checked_div(gas_price).unwrap_or_default();

            if highest_gas_limit > allowance {
                // cap the highest gas limit by max gas caller can afford with given gas price
                highest_gas_limit = allowance;
            }
        }

        // if the provided gas limit is less than computed cap, use that
        let gas_limit = std::cmp::min(U256::from(env.tx.gas_limit), highest_gas_limit);
        env.block.gas_limit = gas_limit;

        // execute the call without writing to db
        let (res, mut env) = transact(&mut db, env)?;
        match res.result {
            ExecutionResult::Success { .. } => {
                // succeeded
            }
            ExecutionResult::Halt { reason, .. } => {
                return match reason {
                    Halt::OutOfGas(_) => Err(InvalidTransactionError::OutOfGas(gas_limit).into()),
                    Halt::NonceOverflow => Err(InvalidTransactionError::NonceMaxValue.into()),
                    err => Err(InvalidTransactionError::EvmHalt(err).into()),
                }
            }
            ExecutionResult::Revert { output, .. } => {
                // if price or limit was included in the request then we can execute the request
                // again with the block's gas limit to check if revert is gas related or not
                return if request_gas.is_some() || request_gas_price.is_some() {
                    let req_gas_limit = env.tx.gas_limit;
                    env.tx.gas_limit = env_gas_limit.try_into().unwrap_or(u64::MAX);
                    let (res, _) = transact(&mut db, env)?;
                    match res.result {
                        ExecutionResult::Success { .. } => {
                            // transaction succeeded by manually increasing the gas limit to
                            // highest, which means the caller lacks funds to pay for the tx
                            Err(InvalidTransactionError::OutOfGas(U256::from(req_gas_limit)).into())
                        }
                        ExecutionResult::Revert { .. } => {
                            // reverted again after bumping the limit
                            Err(InvalidTransactionError::Revert(RevertError::new(output)).into())
                        }
                        ExecutionResult::Halt { reason, .. } => {
                            Err(InvalidTransactionError::EvmHalt(reason).into())
                        }
                    }
                } else {
                    // the transaction did revert
                    Err(InvalidTransactionError::Revert(RevertError::new(output)).into())
                }
            }
        }

        // at this point we know the call succeeded but want to find the _best_ (lowest) gas the
        // transaction succeeds with. we  find this by doing a binary search over the
        // possible range NOTE: this is the gas the transaction used, which is less than the
        // transaction requires to succeed
        let gas_used = res.result.gas_used();
        // the lowest value is capped by the gas it takes for a transfer
        let mut lowest_gas_limit =
            if env.tx.transact_to.is_create() { MIN_CREATE_GAS } else { MIN_TRANSACTION_GAS };
        let mut highest_gas_limit: u64 = highest_gas_limit.try_into().unwrap_or(u64::MAX);
        // pick a point that's close to the estimated gas
        let mut mid_gas_limit = std::cmp::min(
            gas_used * 3,
            ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64,
        );

        let mut last_highest_gas_limit = highest_gas_limit;

        // binary search
        while (highest_gas_limit - lowest_gas_limit) > 1 {
            let mut env = env.clone();
            env.tx.gas_limit = mid_gas_limit;
            let (res, _) = transact(&mut db, env)?;
            match res.result {
                ExecutionResult::Success { .. } => {
                    // cap the highest gas limit with succeeding gas limit
                    highest_gas_limit = mid_gas_limit;
                    // if last two successful estimations only vary by 10%, we consider this to be
                    // sufficiently accurate
                    const ACCURACY: u128 = 10;
                    if (last_highest_gas_limit - highest_gas_limit) as u128 * ACCURACY /
                        (last_highest_gas_limit as u128) <
                        1u128
                    {
                        return Ok(U256::from(highest_gas_limit))
                    }
                    last_highest_gas_limit = highest_gas_limit;
                }
                ExecutionResult::Revert { .. } => {
                    // increase the lowest gas limit
                    lowest_gas_limit = mid_gas_limit;
                }
                ExecutionResult::Halt { reason, .. } => {
                    match reason {
                        Halt::OutOfGas(_) => {
                            // increase the lowest gas limit
                            lowest_gas_limit = mid_gas_limit;
                        }
                        err => {
                            // these should be unreachable because we know the transaction succeeds,
                            // but we consider these cases an error
                            return Err(InvalidTransactionError::EvmHalt(err).into())
                        }
                    }
                }
            }
            // new midpoint
            mid_gas_limit = ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64;
        }

        Ok(U256::from(highest_gas_limit))
    }

    pub(crate) async fn create_access_list_at(
        &self,
        request: CallRequest,
        at: Option<BlockId>,
    ) -> EthResult<AccessList> {
        let block_id = at.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let (mut cfg, block, at) = self.evm_env_at(block_id).await?;
        let state = self.state_at_block_id(at)?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;

        // we want to disable this in eth_call, since this is common practice used by other node
        // impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
        cfg.disable_block_gas_limit = true;

        let mut env = build_call_evm_env(cfg, block, request.clone())?;
        let mut db = SubState::new(State::new(state));

        let from = request.from.unwrap_or_default();
        let to = if let Some(to) = request.to {
            to
        } else {
            let nonce = db.basic(from)?.unwrap_or_default().nonce;
            get_contract_address(from, nonce).into()
        };

        let initial = request.access_list.clone().unwrap_or_default();

        let precompiles = get_precompiles(&env.cfg.spec_id);
        let mut inspector = AccessListInspector::new(initial, from, to, precompiles);
        let (result, _env) = inspect(&mut db, env, &mut inspector)?;

        match result.result {
            ExecutionResult::Halt { reason, .. } => Err(match reason {
                Halt::NonceOverflow => InvalidTransactionError::NonceMaxValue,
                halt => InvalidTransactionError::EvmHalt(halt),
            }),
            ExecutionResult::Revert { output, .. } => {
                Err(InvalidTransactionError::Revert(RevertError::new(output)))
            }
            ExecutionResult::Success { .. } => Ok(()),
        }?;
        Ok(inspector.into_access_list())
    }
}

/// Returns the addresses of the precompiles corresponding to the SpecId.
fn get_precompiles(spec_id: &SpecId) -> Vec<reth_primitives::H160> {
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
pub(crate) fn transact<S>(db: S, env: Env) -> EthResult<(ResultAndState, Env)>
where
    S: Database,
    <S as Database>::Error: Into<EthApiError>,
{
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    let res = evm.transact()?;
    Ok((res, evm.env))
}

/// Executes the [Env] against the given [Database] without committing state changes.
pub(crate) fn inspect<S, I>(db: S, env: Env, inspector: I) -> EthResult<(ResultAndState, Env)>
where
    S: Database,
    <S as Database>::Error: Into<EthApiError>,
    I: Inspector<S>,
{
    let mut evm = revm::EVM::with_env(env);
    evm.database(db);
    let res = evm.inspect(inspector)?;
    Ok((res, evm.env))
}

/// Creates a new [Env] to be used for executing the [CallRequest] in `eth_call`
pub(crate) fn build_call_evm_env(
    mut cfg: CfgEnv,
    block: BlockEnv,
    request: CallRequest,
) -> EthResult<Env> {
    let tx = create_txn_env(&block, request)?;
    Ok(Env { cfg, block, tx })
}

/// Configures a new [TxEnv]  for the [CallRequest]
fn create_txn_env(block_env: &BlockEnv, request: CallRequest) -> EthResult<TxEnv> {
    let CallRequest {
        from,
        to,
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        gas,
        value,
        data,
        nonce,
        access_list,
        chain_id,
    } = request;

    let CallFees { max_priority_fee_per_gas, gas_price } =
        CallFees::ensure_fees(gas_price, max_fee_per_gas, max_priority_fee_per_gas)?;

    let gas_limit = gas.unwrap_or(block_env.gas_limit.min(U256::from(u64::MAX)));

    let env = TxEnv {
        gas_limit: gas_limit.try_into().map_err(|_| InvalidTransactionError::GasUintOverflow)?,
        nonce: nonce
            .map(|n| n.try_into().map_err(|n| InvalidTransactionError::NonceTooHigh))
            .transpose()?,
        caller: from.unwrap_or_default(),
        gas_price,
        gas_priority_fee: max_priority_fee_per_gas,
        transact_to: to.map(TransactTo::Call).unwrap_or_else(TransactTo::create),
        value: value.unwrap_or_default(),
        data: data.map(|data| data.0).unwrap_or_default(),
        chain_id: chain_id.map(|c| c.as_u64()),
        access_list: access_list.map(AccessList::flattened).unwrap_or_default(),
    };

    Ok(env)
}

/// Helper type for representing the fees of a [CallRequest]
struct CallFees {
    /// EIP-1559 priority fee
    max_priority_fee_per_gas: Option<U256>,
    /// Unified gas price setting
    ///
    /// Will be `0` if unset in request
    ///
    /// `gasPrice` for legacy,
    /// `maxFeePerGas` for EIP-1559
    gas_price: U256,
}

// === impl CallFees ===

impl CallFees {
    /// Ensures the fields of a [CallRequest] are not conflicting
    fn ensure_fees(
        call_gas_price: Option<U128>,
        call_max_fee: Option<U128>,
        call_priority_fee: Option<U128>,
    ) -> EthResult<CallFees> {
        match (call_gas_price, call_max_fee, call_priority_fee) {
            (gas_price, None, None) => {
                // request for a legacy transaction
                // set everything to zero
                let gas_price = gas_price.unwrap_or_default();
                Ok(CallFees { gas_price: U256::from(gas_price), max_priority_fee_per_gas: None })
            }
            (None, max_fee_per_gas, max_priority_fee_per_gas) => {
                // request for eip-1559 transaction
                let max_fee = max_fee_per_gas.unwrap_or_default();

                if let Some(max_priority) = max_priority_fee_per_gas {
                    if max_priority > max_fee {
                        // Fail early
                        return Err(
                            // `max_priority_fee_per_gas` is greater than the `max_fee_per_gas`
                            InvalidTransactionError::TipAboveFeeCap.into(),
                        )
                    }
                }
                Ok(CallFees {
                    gas_price: U256::from(max_fee),
                    max_priority_fee_per_gas: max_priority_fee_per_gas.map(U256::from),
                })
            }
            (Some(gas_price), Some(max_fee_per_gas), Some(max_priority_fee_per_gas)) => {
                Err(EthApiError::ConflictingRequestGasPriceAndTipSet {
                    gas_price,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                })
            }
            (Some(gas_price), Some(max_fee_per_gas), None) => {
                Err(EthApiError::ConflictingRequestGasPrice { gas_price, max_fee_per_gas })
            }
            (Some(gas_price), None, Some(max_priority_fee_per_gas)) => {
                Err(EthApiError::RequestLegacyGasPriceAndTipSet {
                    gas_price,
                    max_priority_fee_per_gas,
                })
            }
        }
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
    let mut account_info = db.basic(account)?.unwrap_or_default();

    if let Some(nonce) = account_override.nonce {
        account_info.nonce = nonce;
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
