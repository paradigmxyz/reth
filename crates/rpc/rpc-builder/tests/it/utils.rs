use reth_provider::test_utils::NoopProvider;
use reth_rpc_builder::RpcModuleBuilder;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};

/// Returns an [RpcModuleBuilder] with testing components.
pub fn test_rpc_builder() -> RpcModuleBuilder<NoopProvider, TestPool, ()> {
    RpcModuleBuilder::default().with_client(NoopProvider::default()).with_pool(testing_pool())
}
