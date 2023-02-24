//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

#![allow(unused)] // TODO rm later

use revm::Database;
use crate::{eth::error::EthResult, EthApi};
use reth_primitives::{BlockId, U256};
use reth_provider::{BlockProvider, StateProvider, StateProviderFactory};
use reth_revm::database::{State, SubState};
use reth_rpc_types::CallRequest;
use revm::primitives::{BlockEnv, CfgEnv, Env, ResultAndState};
use crate::eth::error::EthApiError;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
{
    /// Creates the [Env] used by the [EVM](revm::EVM) when executing a call.
    fn build_call_env(&self, request: CallRequest, block_env: BlockEnv) -> Env {
        todo!()
    }

    /// Executes the call request with the provided database and evm environment without committing any changes to the
    /// database
    pub(crate) fn call_with_db_and_env<S>(
        &self,
        request: CallRequest,
        db: S,
        block_env: BlockEnv,
        cfg_env: CfgEnv
    ) -> EthResult<ResultAndState>
    where
        S: Database,
        <S as Database>::Error: Into<EthApiError>
    {
        // the revm database impl

        // initialize the db
        let mut evm = revm::EVM::new();
        evm.env.cfg = cfg_env;
        evm.env.block = block_env;
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
        let mut db = SubState::new(State::new(state));
        // binary search over range to find best gas

        todo!()
    }
}
