//! utilities for working with revm

use crate::eth::error::{EthApiError, EthResult, InvalidTransactionError};
use reth_primitives::{AccessList, Address, U256};
use reth_rpc_types::CallRequest;
use revm::{
    precompile::{Precompiles, SpecId as PrecompilesSpecId},
    primitives::{BlockEnv, CfgEnv, Env, ResultAndState, SpecId, TransactTo, TxEnv},
    Database, Inspector,
};

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
    cfg: CfgEnv,
    block: BlockEnv,
    request: CallRequest,
) -> EthResult<Env> {
    let tx = create_txn_env(&block, request)?;
    Ok(Env { cfg, block, tx })
}

/// Configures a new [TxEnv]  for the [CallRequest]
pub(crate) fn create_txn_env(block_env: &BlockEnv, request: CallRequest) -> EthResult<TxEnv> {
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

    let CallFees { max_priority_fee_per_gas, gas_price } = CallFees::ensure_fees(
        gas_price,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        block_env.basefee,
    )?;

    let gas_limit = gas.unwrap_or(block_env.gas_limit.min(U256::from(u64::MAX)));

    let env = TxEnv {
        gas_limit: gas_limit.try_into().map_err(|_| InvalidTransactionError::GasUintOverflow)?,
        nonce: nonce
            .map(|n| n.try_into().map_err(|_| InvalidTransactionError::NonceTooHigh))
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
    /// If no `gasPrice` or `maxFeePerGas` is set, then the `gas_price` in the response will
    /// fallback to the given `basefee`.
    fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        base_fee: U256,
    ) -> EthResult<CallFees> {
        match (call_gas_price, call_max_fee, call_priority_fee) {
            (gas_price, None, None) => {
                // request for a legacy transaction
                // set everything to zero
                let gas_price = gas_price.unwrap_or(base_fee);
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
                            InvalidTransactionError::TipAboveFeeCap.into(),
                        )
                    }
                }
                Ok(CallFees { gas_price: max_fee, max_priority_fee_per_gas })
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
