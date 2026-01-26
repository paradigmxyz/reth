#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy_op_evm::block::OpAlloyReceiptBuilder;
    use alloy_op_hardforks::OpChainHardforks;
    use eyre::Ok;
    use reth_chainspec::Head;
    use reth_db::open_db_read_only;
    use reth_evm::{execute::BlockExecutorFactory, EvmEnv, EvmFactory};
    use reth_evm_ethereum::MockExecutor;
    use reth_node_builder::{
        common::WithConfigs,
        components::{PayloadBuilderBuilder, PoolBuilder},
        BuilderContext, Node, NodeConfig,
    };
    use reth_optimism_chainspec::{OpChainSpecBuilder, OP_MAINNET};
    use reth_optimism_evm::{
        OpBlockExecutionCtx, OpBlockExecutorFactory, OpEvmConfig, OpEvmFactory,
    };
    use reth_optimism_node::{
        node::OpPayloadBuilder, OpDAConfig, OpFullNodeTypes, OpNode, OpPoolBuilder,
    };
    use reth_provider::providers::{RocksDBProvider, StaticFileProvider};
    use reth_revm::{
        db::{CacheDB, EmptyDB},
        State,
    };
    use reth_tasks::{TaskExecutor, TokioTaskExecutor};

    #[tokio::test]
    async fn mock_payload_builder() -> eyre::Result<()> {
        let best_transactions = ();
        let da_cfg = OpDAConfig::new(42, 42);
        let evm_config = OpEvmConfig::optimism(OP_MAINNET.clone());

        let executor = TaskExecutor::current();
        let cfg_container =
            WithConfigs { config: NodeConfig::test(), toml_config: reth_config::Config::default() };

        let factory = OpNode::provider_factory_builder()
            .open_read_only(OP_MAINNET.clone(), "datadir")
            .unwrap();

        let provider = factory.provider().unwrap();

        let ctx = BuilderContext::new(Head::default(), provider, executor, cfg_container);

        let pool = OpPoolBuilder::default().build_pool(&ctx, evm_config).await?;

        let pb = OpPayloadBuilder::new(false)
            .with_transactions(best_transactions)
            .with_da_config(da_cfg)
            .build_payload_builder(&ctx, pool, evm_config)
            .await?;

        Ok(())

        // now we can write tests
    }
}
