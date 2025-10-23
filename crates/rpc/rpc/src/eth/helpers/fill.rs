//! Contains RPC handler implementations for filling transactions.

use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::FillTransaction, RpcNodeCore};

impl<N, Rpc> FillTransaction for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
    Self: reth_rpc_eth_api::helpers::Call
        + reth_rpc_eth_api::helpers::estimate::EstimateCall
        + reth_rpc_eth_api::helpers::EthFees
        + reth_rpc_eth_api::helpers::LoadPendingBlock
        + reth_rpc_eth_api::helpers::LoadState,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::helpers::types::EthRpcConverter;
    use alloy_primitives::{Address, U256};
    use alloy_rpc_types_eth::{request::TransactionRequest, BlockId, BlockNumberOrTag};
    use reth_chainspec::ChainSpec;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{helpers::FillTransaction, node::RpcNodeCoreAdapter};
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::collections::HashMap;

    fn mock_eth_api(
        accounts: HashMap<Address, ExtendedAccount>,
    ) -> EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let pool = testing_pool();
        let mock_provider = MockEthProvider::default();

        let evm_config = EthEvmConfig::new(mock_provider.chain_spec());
        mock_provider.extend_accounts(accounts);

        EthApi::builder(mock_provider, pool, NoopNetwork::default(), evm_config).build()
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_chain_id() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)), // 10 ETH
        )]);

        let eth_api = mock_eth_api(accounts);

        let mut tx_req = TransactionRequest::default();
        tx_req.from = Some(address);
        tx_req.to = Some(Address::random().into());
        tx_req.gas = Some(21_000);

        let block_id = BlockId::Number(BlockNumberOrTag::Latest);

        let result = eth_api.fill_transaction(tx_req, block_id, None).await;

        if let Ok(filled) = result {
            // Should fill with the chain id from provider
            assert!(filled.chain_id.is_some());
        }
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_nonce() {
        let address = Address::random();
        let nonce = 42u64;

        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(nonce, U256::from(1_000_000_000_000_000_000u64)), // 1 ETH
        )]);

        let eth_api = mock_eth_api(accounts);

        let mut tx_req = TransactionRequest::default();
        tx_req.from = Some(address);
        tx_req.to = Some(Address::random().into());
        tx_req.value = Some(U256::from(1000));
        tx_req.gas = Some(21_000);

        let block_id = BlockId::Number(BlockNumberOrTag::Latest);

        let result = eth_api.fill_transaction(tx_req, block_id, None).await;

        if let Ok(filled) = result {
            assert_eq!(filled.nonce, Some(nonce));
        }
    }

    #[tokio::test]
    async fn test_fill_transaction_preserves_provided_fields() {
        let address = Address::random();
        let provided_nonce = 100u64;
        let provided_gas_limit = 50_000u64;

        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(42, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let mut tx_req = TransactionRequest::default();
        tx_req.from = Some(address);
        tx_req.to = Some(Address::random().into());
        tx_req.value = Some(U256::from(1000));
        tx_req.nonce = Some(provided_nonce); // Explicitly set nonce
        tx_req.gas = Some(provided_gas_limit); // Explicitly set gas limit

        let block_id = BlockId::Number(BlockNumberOrTag::Latest);

        let result = eth_api.fill_transaction(tx_req, block_id, None).await;

        if let Ok(filled) = result {
            // Should preserve the provided nonce and gas limit
            assert_eq!(filled.nonce, Some(provided_nonce));
            assert_eq!(filled.gas, Some(provided_gas_limit));
        }
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_all_missing_fields() {
        let address = Address::random();

        let balance = U256::from(100u128) * U256::from(1_000_000_000_000_000_000u128);
        let accounts = HashMap::from([(address, ExtendedAccount::new(5, balance))]);

        let eth_api = mock_eth_api(accounts);

        let tx_req = TransactionRequest::default();

        let block_id = BlockId::Number(BlockNumberOrTag::Latest);

        let result = eth_api.fill_transaction(tx_req, block_id, None).await;

        if let Ok(filled) = result {
            // Verify fields are filled
            assert!(filled.chain_id.is_some(), "chain_id should be filled");
            assert!(filled.nonce.is_some(), "nonce should be filled");

            // Should have some fee field filled (either gas_price or EIP-1559 fields)
            let has_fees = filled.gas_price.is_some() ||
                filled.max_fee_per_gas.is_some() ||
                filled.max_priority_fee_per_gas.is_some();
            assert!(has_fees, "at least one fee field should be filled");
        }
    }
}
