use alloy_primitives::U256;
use reth_node_api::FullNodeComponents;
use reth_provider::TransactionsProvider;
use reth_rpc_eth_api::{helpers::EthApiSpec, RpcNodeCore};

use crate::EthApi;

impl<Components> EthApiSpec for EthApi<Components>
where
    Components: FullNodeComponents,
{
    type Transaction = <Components::Provider as TransactionsProvider>::Transaction;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(
        &self,
    ) -> &parking_lot::RwLock<Vec<Box<dyn reth_rpc_eth_api::helpers::EthSigner<Self::Transaction>>>>
    {
        self.inner.signers()
    }
}
