//! Contains RPC handler implementations specific to state.

use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthState, LoadPendingBlock, LoadState},
    RpcNodeCore,
};

impl<N, Rpc> EthState for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
    Self: LoadPendingBlock,
{
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_proof_window()
    }
}

impl<N, Rpc> LoadState for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
    Self: LoadPendingBlock,
{
}

#[cfg(test)]
mod tests {
    use crate::eth::helpers::types::EthRpcConverter;

    use super::*;
    use alloy_primitives::{Address, StorageKey, StorageValue, U256};
    use reth_chainspec::ChainSpec;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider, NoopProvider},
        ChainSpecProvider,
    };
    use reth_rpc_eth_api::{helpers::EthState, node::RpcNodeCoreAdapter};
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};
    use std::collections::HashMap;

    fn noop_eth_api() -> EthApi<
        RpcNodeCoreAdapter<NoopProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    > {
        let provider = NoopProvider::default();
        let pool = testing_pool();
        let evm_config = EthEvmConfig::mainnet();

        EthApi::builder(provider, pool, NoopNetwork::default(), evm_config).build()
    }

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
    async fn test_storage() {
        // === Noop ===
        let eth_api = noop_eth_api();
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO.into(), None).await.unwrap();
        assert_eq!(storage, U256::ZERO.to_be_bytes());

        // === Mock ===
        let storage_value = StorageValue::from(1337);
        let storage_key = StorageKey::random();
        let storage = HashMap::from([(storage_key, storage_value)]);

        let accounts =
            HashMap::from([(address, ExtendedAccount::new(0, U256::ZERO).extend_storage(storage))]);
        let eth_api = mock_eth_api(accounts);

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).await.unwrap();
        assert_eq!(storage, storage_value.to_be_bytes());
    }

    #[tokio::test]
    async fn test_get_account_missing() {
        let eth_api = noop_eth_api();
        let address = Address::random();
        let account = eth_api.get_account(address, Default::default()).await.unwrap();
        assert!(account.is_none());
    }
}
