use reth_evm::ConfigureEvm;
use reth_primitives::{
    revm_primitives::{BlockEnv, OptimismFields, TxEnv},
    Bytes,
};
use reth_rpc_eth_api::{
    helpers::{Call, EthCall},
    EthApiTypes, FromEthApiError,
};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types::TransactionRequest;

use crate::OpEthApi;

impl<Eth: EthCall> EthCall for OpEthApi<Eth> where EthApiError: From<Eth::Error> {}

impl<Eth> Call for OpEthApi<Eth>
where
    Eth: Call + EthApiTypes,
    EthApiError: From<Eth::Error>,
{
    fn call_gas_limit(&self) -> u64 {
        self.inner.call_gas_limit()
    }

    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }

    fn create_txn_env(
        &self,
        block_env: &BlockEnv,
        request: TransactionRequest,
    ) -> Result<TxEnv, Self::Error> {
        let mut env =
            self.inner.create_txn_env(block_env, request).map_err(Self::Error::from_eth_err)?;

        env.optimism = OptimismFields { enveloped_tx: Some(Bytes::new()), ..Default::default() };

        Ok(env)
    }
}
