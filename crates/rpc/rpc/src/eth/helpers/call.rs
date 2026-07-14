//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use alloy_primitives::U256;
use evm2::ethereum::RecoveredTxEnvelope;
use reth_evm::{Database, EvmEnvFor, TxEnvFor};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, RpcInvalidTransactionError};
use std::borrow::Borrow;

impl<N, Rpc> EthCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: Borrow<RecoveredTxEnvelope>,
{
}

impl<N, Rpc> Call for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: Borrow<RecoveredTxEnvelope>,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.max_simulate_blocks()
    }

    #[inline]
    fn compute_state_root_for_eth_simulate(&self) -> bool {
        self.inner.compute_state_root_for_eth_simulate()
    }

    #[inline]
    fn evm_memory_limit(&self) -> u64 {
        self.inner.evm_memory_limit()
    }

    fn caller_gas_allowance(
        &self,
        mut db: impl Database<Error: Into<EthApiError>>,
        _evm_env: &EvmEnvFor<Self::Evm>,
        tx_env: &TxEnvFor<Self::Evm>,
    ) -> Result<u64, Self::Error> {
        let tx = tx_env.borrow();
        let gas_price = tx.effective_gas_price(None);
        let balance = db
            .get_account(&tx.signer())
            .map_err(Into::into)?
            .map(|account| account.balance)
            .unwrap_or_default();
        let value = tx.value();
        let allowance = balance.checked_sub(value).ok_or_else(|| {
            EthApiError::from(RpcInvalidTransactionError::InsufficientFunds {
                cost: value,
                balance,
            })
        })? / U256::from(gas_price);

        Ok(allowance.try_into().unwrap_or(u64::MAX))
    }
}

impl<N, Rpc> EstimateCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: Borrow<RecoveredTxEnvelope>,
{
}
