//! Contains RPC handler implementations specific to tracing.

use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{Call, Trace},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

impl<N, Rpc> Trace for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
    Self: Call,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::helpers::types::EthRpcConverter;
    use alloy_eips::BlockId;
    use evm2_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
    use reth_chainspec::ChainSpec;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::test_utils::testing_pool;

    #[tokio::test]
    async fn canonical_transaction_index_out_of_bounds_is_invalid_params() {
        let eth_api = EthApi::<_, EthRpcConverter<ChainSpec>>::builder(
            NoopProvider::default(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::mainnet(),
        )
        .build();
        let block = RecoveredBlock::new_unhashed(Block::default(), vec![]);

        let err = eth_api
            .spawn_with_state_at_block(BlockId::latest(), move |this, mut db| {
                let inspector = TracingInspector::new(TracingInspectorConfig::default_parity());
                this.inspect_canonical_transaction_in_block(&block, &mut db, inspector, 0, None)
                    .map(|_| ())
            })
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "tx_index 0 out of bounds for block with 0 transactions");
    }
}
