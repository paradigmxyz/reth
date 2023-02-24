//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

#![allow(unused)] // TODO rm later

use crate::{
    eth::error::{EthApiError, EthResult, InvalidTransactionError},
    EthApi,
};
use reth_primitives::{AccessList, Address, BlockId, Bytes, TransactionKind, U128, U256};
use reth_provider::{BlockProvider, StateProvider, StateProviderFactory};
use reth_revm::database::{State, SubState};
use reth_rpc_types::CallRequest;
use revm::{
    primitives::{ruint::Uint, BlockEnv, CfgEnv, Env, ResultAndState, TransactTo, TxEnv},
    Database,
};

// Gas per transaction not creating a contract.
pub(crate) const MIN_TRANSACTION_GAS: U256 = Uint::from_limbs([21_000, 0, 0, 0]);

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    /// Executes the call request at the given [BlockId]
    pub(crate) fn call_at(
        &self,
        request: CallRequest,
        at: BlockId,
    ) -> EthResult<(ResultAndState, Env)> {
        todo!()
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
    ) -> EthResult<(ResultAndState, Env)>
    where
        S: StateProvider,
    {
        // we want to disable this in eth_call, since this is common practice used by other node
        // impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
        cfg.disable_block_gas_limit = true;

        let mut env = build_call_evm_env(cfg, block, request)?;
        let mut db = SubState::new(State::new(state));
        transact(&mut db, env)
    }

    /// Estimate gas needed for execution of the `request` at the [BlockId] .
    pub(crate) fn estimate_gas_at(&self, mut request: CallRequest, at: BlockId) -> EthResult<U256> {
        // TODO get a StateProvider for the given blockId and BlockEnv
        todo!()
    }

    /// Estimates the gas usage of the `request` with the state.
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
        // get the highest possible gas limit, either the request's set value or the currently
        // configured gas limit
        let mut highest_gas_limit = request.gas.unwrap_or(block.gas_limit);

        // Configure the evm env
        let mut env = build_call_evm_env(cfg, block, request)?;

        // if the request is a simple transfer we can optimize
        if env.tx.data.is_empty() {
            if let TransactTo::Call(to) = env.tx.transact_to {
                let no_code = state.account_code(to)?.map(|code| code.is_empty()).unwrap_or(true);
                if no_code {
                    return Ok(MIN_TRANSACTION_GAS)
                }
            }
        }

        let mut db = SubState::new(State::new(state));

        todo!()
    }
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

    let gas_limit = gas.unwrap_or(block_env.gas_limit);

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
