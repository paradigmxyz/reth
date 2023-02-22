//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

#![allow(unused)] // TODO rm later

use crate::{eth::error::EthResult, EthApi};
use reth_executor::revm_wrap::{State, SubState};
use reth_primitives::{BlockId, U256};
use reth_provider::{BlockProvider, StateProvider, StateProviderFactory};
use reth_rpc_types::CallRequest;
use revm::primitives::{BlockEnv, Env, ResultAndState};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    /// Creates the [Env] used by the [EVM](revm::EVM) when executing a call.
    fn build_call_env(&self, request: CallRequest, block_env: BlockEnv) -> Env {
        todo!()
    }

    /// Executes the call request against the given `state` without committing any changes to the
    /// database
    pub(crate) fn call_with<S>(
        &self,
        request: CallRequest,
        state: S,
        block_env: BlockEnv,
        // TODO add overrides
    ) -> EthResult<ResultAndState>
    where
        S: StateProvider,
    {
        // the revm database impl
        let mut db = SubState::new(State::new(state));
        let mut evm = revm::EVM::new();
        evm.env = self.build_call_env(request, block_env);
        evm.database(db);
        // TODO error conversion from EMVError to EthApiErr
        let res = evm.transact_ref().unwrap();
        Ok(res)
    }

    /// Estimate gas needed for execution of the `request` at the [BlockId] .
    pub(crate) fn estimate_gas_at(&self, mut request: CallRequest, at: BlockId) -> EthResult<U256> {
        // TODO get a StateProvider for the given blockId and BlockEnv
        todo!()
    }

    /// Estimates the gas usage of the `request` with the state.
    fn estimate_gas_with<S>(
        &self,
        mut request: CallRequest,
        state: S,
        block_env: BlockEnv,
    ) -> EthResult<U256>
    where
        S: StateProvider,
    {
        // TODO: check if transfer
        // binary search over range to find best gas

        todo!()
    }
}
