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

        use alloy_consensus::Header;
        use alloy_primitives::B256;

        let mut genesis_header = Header::default();
        genesis_header.number = 0;
        genesis_header.gas_limit = 30_000_000;
        genesis_header.timestamp = 1;

        let genesis_hash = B256::ZERO;
        mock_provider.add_header(genesis_hash, genesis_header);

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

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Should fill with the chain id from provider
        assert!(filled.chain_id.is_some());
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

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        assert_eq!(filled.nonce, Some(nonce));
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

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Should preserve the provided nonce and gas limit
        assert_eq!(filled.nonce, Some(provided_nonce));
        assert_eq!(filled.gas, Some(provided_gas_limit));
    }

    #[tokio::test]
    async fn test_fill_transaction_fills_all_missing_fields() {
        let address = Address::random();

        let balance = U256::from(100u128) * U256::from(1_000_000_000_000_000_000u128);
        let accounts = HashMap::from([(address, ExtendedAccount::new(5, balance))]);

        let eth_api = mock_eth_api(accounts);

        // Create a simple transfer transaction
        let mut tx_req = TransactionRequest::default();
        tx_req.from = Some(address);
        tx_req.to = Some(Address::random().into());

        let block_id = BlockId::Number(BlockNumberOrTag::Latest);

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Verify fields are filled
        assert!(filled.chain_id.is_some(), "chain_id should be filled");
        assert!(filled.nonce.is_some(), "nonce should be filled");

        // Should have some fee field filled (either gas_price or EIP-1559 fields)
        let has_fees = filled.gas_price.is_some() ||
            filled.max_fee_per_gas.is_some() ||
            filled.max_priority_fee_per_gas.is_some();
        assert!(has_fees, "at least one fee field should be filled");

        // Gas limit should be filled
        assert!(filled.gas.is_some(), "gas limit should be filled");
    }
}
