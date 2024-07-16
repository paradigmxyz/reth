use reth_evm::ConfigureEvm;
use reth_primitives::{
    revm_primitives::{BlockEnv, OptimismFields, TxEnv},
    Bytes,
};
use reth_rpc_eth_api::helpers::Call;
use reth_rpc_eth_types::EthResult;
use reth_rpc_types::TransactionRequest;

use crate::OpEthApi;

impl<Eth: Call> Call for OpEthApi<Eth> {
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
    ) -> EthResult<TxEnv> {
        let mut env = Eth::create_txn_env(&self.inner, block_env, request)?;

        env.optimism = OptimismFields { enveloped_tx: Some(Bytes::new()), ..Default::default() };

        Ok(env)
    }
}
