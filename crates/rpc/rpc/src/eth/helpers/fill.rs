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

        let genesis_header =
            Header { number: 0, gas_limit: 30_000_000, timestamp: 1, ..Default::default() };

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

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            gas: Some(21_000),
            ..Default::default()
        };

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

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            value: Some(U256::from(1000)),
            gas: Some(21_000),
            ..Default::default()
        };

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

        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            value: Some(U256::from(1000)),
            nonce: Some(provided_nonce),
            gas: Some(provided_gas_limit),
            ..Default::default()
        };

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
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            ..Default::default()
        };

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

    #[tokio::test]
    async fn test_fill_transaction_legacy_gas_price() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // Legacy transaction (type 0)
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(0),
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Legacy transaction should have gas_price filled, not EIP-1559 fields
        assert!(filled.gas_price.is_some(), "gas_price should be filled for legacy tx");
        assert!(filled.max_fee_per_gas.is_none(), "max_fee_per_gas should not be set for legacy");
        assert!(
            filled.max_priority_fee_per_gas.is_none(),
            "max_priority_fee_per_gas should not be set for legacy"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_eip2930_gas_price() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // EIP-2930 transaction (type 1)
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(1),
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // EIP-2930 transaction should have gas_price filled, not EIP-1559 fields
        assert!(filled.gas_price.is_some(), "gas_price should be filled for EIP-2930 tx");
        assert!(filled.max_fee_per_gas.is_none(), "max_fee_per_gas should not be set for EIP-2930");
        assert!(
            filled.max_priority_fee_per_gas.is_none(),
            "max_priority_fee_per_gas should not be set for EIP-2930"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_eip1559_fees_error_when_conflicting() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // Legacy transaction type but EIP-1559 fees set (conflict)
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(0),         // Legacy
            max_fee_per_gas: Some(1000000000), // But has EIP-1559 field!
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let result = eth_api.fill_transaction(tx_req, block_id, None).await;

        // Should error because legacy tx can't have EIP-1559 fields
        assert!(result.is_err(), "should error on conflicting fee fields");
    }

    #[tokio::test]
    async fn test_fill_transaction_eip4844_blob_fee() {
        use alloy_primitives::B256;

        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // EIP-4844 blob transaction with versioned hashes but no blob fee
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(3), // EIP-4844
            blob_versioned_hashes: Some(vec![B256::ZERO]),
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Blob transaction should have max_fee_per_blob_gas filled
        assert!(
            filled.max_fee_per_blob_gas.is_some(),
            "max_fee_per_blob_gas should be filled for blob tx"
        );
        assert!(
            filled.blob_versioned_hashes.is_some(),
            "blob_versioned_hashes should be preserved"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_eip4844_preserves_blob_fee() {
        use alloy_primitives::B256;

        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        let provided_blob_fee = 5000000u128;

        // EIP-4844 blob transaction with blob fee already set
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(3), // EIP-4844
            blob_versioned_hashes: Some(vec![B256::ZERO]),
            max_fee_per_blob_gas: Some(provided_blob_fee), // Already set
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Should preserve the provided blob fee
        assert_eq!(
            filled.max_fee_per_blob_gas,
            Some(provided_blob_fee),
            "should preserve provided max_fee_per_blob_gas"
        );
    }

    #[tokio::test]
    async fn test_fill_transaction_non_blob_tx_no_blob_fee() {
        let address = Address::random();
        let accounts = HashMap::from([(
            address,
            ExtendedAccount::new(0, U256::from(10_000_000_000_000_000_000u64)),
        )]);

        let eth_api = mock_eth_api(accounts);

        // EIP-1559 transaction without blob fields
        let tx_req = TransactionRequest {
            from: Some(address),
            to: Some(Address::random().into()),
            transaction_type: Some(2), // EIP-1559
            ..Default::default()
        };

        let block_id = BlockId::Number(BlockNumberOrTag::Number(0));

        let filled = eth_api
            .fill_transaction(tx_req, block_id, None)
            .await
            .expect("fill_transaction should succeed");

        // Non-blob transaction should NOT have blob fee filled
        assert!(
            filled.max_fee_per_blob_gas.is_none(),
            "max_fee_per_blob_gas should not be set for non-blob tx"
        );
    }
}
