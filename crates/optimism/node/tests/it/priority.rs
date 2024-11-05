//! Node builder test that customizes priority of transactions in the block.
use reth_db::test_utils::create_test_rw_db;
use reth_node_api::{FullNodeTypes, NodeTypesWithEngine};
use reth_node_builder::{components::ComponentsBuilder, NodeBuilder, NodeConfig};
use reth_optimism_chainspec::{OpChainSpec, OP_DEV};
use reth_optimism_node::{
    args::RollupArgs,
    node::{
        OpConsensusBuilder, OpExecutorBuilder, OpNetworkBuilder, OpPayloadBuilder, OpPoolBuilder,
    },
    OpEngineTypes, OptimismNode,
};
use reth_optimism_payload_builder::builder::OpPayloadTransactions;

#[derive(Clone)]
struct CustomTxPriority {}

impl OpPayloadTransactions for CustomTxPriority {
    fn best_transactions<Pool>(
        &self,
        pool: Pool,
        attr: reth_transaction_pool::BestTransactionsAttributes,
        // TODO(Seva): More arguments in this trait implementation for PayloadAttributes, etc.
    ) -> reth_transaction_pool::BestTransactionsFor<Pool>
    where
        Pool: reth_transaction_pool::TransactionPool,
    {
        // TODO(Seva): Build a custom implementation here
        pool.best_transactions_with_attributes(attr)
    }
}

fn build_components<Node>() -> ComponentsBuilder<
    Node,
    OpPoolBuilder,
    OpPayloadBuilder<CustomTxPriority>,
    OpNetworkBuilder,
    OpExecutorBuilder,
    OpConsensusBuilder,
>
where
    Node:
        FullNodeTypes<Types: NodeTypesWithEngine<Engine = OpEngineTypes, ChainSpec = OpChainSpec>>,
{
    let RollupArgs { disable_txpool_gossip, compute_pending_block, discovery_v4, .. } =
        RollupArgs::default();
    ComponentsBuilder::default()
        .node_types::<Node>()
        .pool(OpPoolBuilder::default())
        .payload(
            OpPayloadBuilder::new(compute_pending_block).with_transactions(CustomTxPriority {}),
        )
        .network(OpNetworkBuilder { disable_txpool_gossip, disable_discovery_v4: !discovery_v4 })
        .executor(OpExecutorBuilder::default())
        .consensus(OpConsensusBuilder::default())
}

#[tokio::test]
async fn test_custom_block_priority_config() {
    let config = NodeConfig::new(OP_DEV.clone());
    let db = create_test_rw_db();

    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<OptimismNode>()
        .with_components(build_components())
        .check_launch();

    // TODO(Seva): Launch it for real and test the custom priority
}
