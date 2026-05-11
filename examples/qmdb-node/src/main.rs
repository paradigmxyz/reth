//! Reth SDK node that computes execution state roots with QMDb.

#![warn(unused_crate_dependencies)]

use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_engine_primitives::{ConfigureEngineEvm, NoopInvalidBlockHook, PayloadValidator};
use reth_engine_tree::{
    persistence::{RemoveBlocksHook, SaveBlocksHook},
    tree::{payload_validator::CustomStateRootInput, BasicEngineValidator, TreeConfig},
};
use reth_ethereum::{
    chainspec::{ChainSpec, EthChainSpec},
    cli::interface::Cli,
    evm::primitives::{ConfigureEvm, NextBlockEnvAttributes},
    node::{
        api::{BlockTy, FullNodeComponents, FullNodeTypes, NodeTypes, PayloadTypes},
        builder::{components::PayloadServiceBuilder, AddOnsContext, BuilderContext},
        node::{EthereumAddOns, EthereumEngineValidatorBuilder},
        EthEngineTypes, EthereumNode,
    },
    pool::{PoolTransaction, TransactionPool},
    provider::{
        AccountReader, BlockNumReader, CanonStateSubscriptions, ChangeSetReader, HeaderProvider,
        ProviderError, StateReader, StorageChangeSetReader, StorageReader,
    },
    storage::StateProviderBox,
    trie::{updates::TrieUpdates, HashedPostState},
    EthPrimitives, TransactionSigned,
};
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumPayloadBuilder};
use reth_node_builder::{
    rpc::{EngineValidatorBuilder, PayloadValidatorBuilder},
    PayloadBuilderConfig,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_qmdb::{
    genesis_hashed_state, QmdbBlock, QmdbConfig, QmdbStage, QmdbState, QmdbStateProviderFactory,
    QmdbStateRootProvider,
};
use reth_stages::{StageId, StageSetBuilder};
use reth_trie_db::ChangesetCache;
use std::{
    fmt,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Default)]
struct QmdbStateLoader {
    state: Arc<OnceLock<QmdbState>>,
}

impl fmt::Debug for QmdbStateLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QmdbStateLoader").finish_non_exhaustive()
    }
}

impl QmdbStateLoader {
    fn open<ChainSpec>(
        &self,
        config: &reth_ethereum::node::core::node_config::NodeConfig<ChainSpec>,
    ) -> eyre::Result<QmdbState>
    where
        ChainSpec: EthChainSpec,
    {
        if let Some(state) = self.state.get() {
            return Ok(state.clone());
        }

        let path = config.datadir().data_dir().join("qmdb");
        let state = QmdbState::open(QmdbConfig::new(path).with_partition_prefix("state"))?;
        let _ = self.state.set(state);
        Ok(self.state.get().expect("QMDb state was just initialized").clone())
    }

    fn open_for_provider<ChainSpec, Provider>(
        &self,
        config: &reth_ethereum::node::core::node_config::NodeConfig<ChainSpec>,
        provider: &Provider,
    ) -> eyre::Result<QmdbState>
    where
        ChainSpec: EthChainSpec,
        Provider: BlockNumReader + HeaderProvider,
    {
        let state = self.open_initialized(config)?;
        state.reconcile_canonical(provider)?;
        Ok(state)
    }

    fn open_initialized<ChainSpec>(
        &self,
        config: &reth_ethereum::node::core::node_config::NodeConfig<ChainSpec>,
    ) -> eyre::Result<QmdbState>
    where
        ChainSpec: EthChainSpec,
    {
        let state = self.open(config)?;
        if state.head()?.is_none() {
            let genesis = config.chain.genesis_header();
            state.commit_block(
                QmdbBlock {
                    number: genesis.number(),
                    hash: config.chain.genesis_hash(),
                    parent_hash: genesis.parent_hash(),
                },
                genesis_hashed_state(config.chain.genesis()),
            )?;
        }
        Ok(state)
    }
}

#[derive(Debug, Clone, Default)]
struct QmdbPayloadBuilder {
    qmdb: QmdbStateLoader,
}

impl QmdbPayloadBuilder {
    const fn new(qmdb: QmdbStateLoader) -> Self {
        Self { qmdb }
    }
}

impl<Node, Pool, Evm> PayloadServiceBuilder<Node, Pool, Evm> for QmdbPayloadBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            Payload = EthEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
        >,
    >,
    Node::Provider: CanonStateSubscriptions + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>
        + Unpin
        + 'static,
    Evm: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>
        + 'static,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: Evm,
    ) -> eyre::Result<PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload>> {
        let qmdb = self.qmdb.open_for_provider(ctx.config(), ctx.provider())?;
        let provider = QmdbStateProviderFactory::new(ctx.provider().clone(), qmdb);
        let payload_builder = EthereumPayloadBuilder::new(
            provider,
            pool,
            evm_config,
            EthereumBuilderConfig::new().with_extra_data(ctx.payload_builder_config().extra_data()),
        );

        let conf = ctx.payload_builder_config();
        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            ctx.task_executor().clone(),
            payload_job_config,
            payload_builder,
        );

        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical_task("qmdb payload builder service", payload_service);

        Ok(payload_builder)
    }
}

#[derive(Debug, Clone)]
struct QmdbEngineValidatorBuilder<EV> {
    payload_validator_builder: EV,
    qmdb: QmdbStateLoader,
}

impl<EV> QmdbEngineValidatorBuilder<EV> {
    const fn new(payload_validator_builder: EV, qmdb: QmdbStateLoader) -> Self {
        Self { payload_validator_builder, qmdb }
    }
}

impl<Node, EV> EngineValidatorBuilder<Node> for QmdbEngineValidatorBuilder<EV>
where
    Node: FullNodeComponents<
        Evm: ConfigureEngineEvm<
            <<Node::Types as NodeTypes>::Payload as PayloadTypes>::ExecutionData,
        >,
        Types: NodeTypes<ChainSpec: EthChainSpec, Primitives = EthPrimitives>,
    >,
    EV: PayloadValidatorBuilder<Node>,
    EV::Validator:
        PayloadValidator<<Node::Types as NodeTypes>::Payload, Block = BlockTy<Node::Types>> + Clone,
{
    type EngineValidator = BasicEngineValidator<Node::Provider, Node::Evm, EV::Validator>;

    async fn build_tree_validator(
        self,
        ctx: &AddOnsContext<'_, Node>,
        tree_config: TreeConfig,
        changeset_cache: ChangesetCache,
    ) -> eyre::Result<Self::EngineValidator> {
        let validator = self.payload_validator_builder.build(ctx).await?;
        let qmdb = self.qmdb.open_for_provider(ctx.config, ctx.node.provider())?;
        let save_qmdb = qmdb.clone();
        let save_blocks_hook: SaveBlocksHook<EthPrimitives> = Arc::new(move |blocks| {
            let mut qmdb_blocks = Vec::with_capacity(blocks.len());
            for block in blocks {
                let recovered = block.recovered_block();
                let header = recovered.header();
                qmdb_blocks.push((
                    QmdbBlock {
                        number: header.number(),
                        hash: recovered.hash(),
                        parent_hash: header.parent_hash(),
                    },
                    HashedPostState::from((*block.hashed_state()).clone()),
                ));
            }
            if let Some((first, _)) = qmdb_blocks.first() &&
                let Some(head) = save_qmdb.head().map_err(ProviderError::other)? &&
                (head.number >= first.number || head.hash != first.parent_hash)
            {
                save_qmdb
                    .rewind_to_block(first.number.saturating_sub(1))
                    .map_err(ProviderError::other)?;
            }
            save_qmdb.commit_blocks(qmdb_blocks).map(|_| ()).map_err(ProviderError::other)
        });
        let remove_qmdb = qmdb.clone();
        let remove_blocks_hook: RemoveBlocksHook = Arc::new(move |new_tip| {
            remove_qmdb.rewind_to_block(new_tip).map(|_| ()).map_err(ProviderError::other)
        });

        let root_qmdb = qmdb.clone();
        let custom_state_root = Arc::new(move |input: CustomStateRootInput<'_, EthPrimitives>| {
            root_qmdb
                .overlay_root(input.hashed_state.get().as_ref().clone())
                .map(|commit| (commit.root, TrieUpdates::default()))
                .map_err(ProviderError::other)
        });

        Ok(BasicEngineValidator::new(
            ctx.node.provider().clone(),
            Arc::new(ctx.node.consensus().clone()),
            ctx.node.evm_config().clone(),
            validator,
            tree_config,
            Box::new(NoopInvalidBlockHook::default()),
            changeset_cache,
            ctx.node.task_executor().clone(),
        )
        .with_custom_state_root(custom_state_root)
        .with_persistence_hooks(Some(save_blocks_hook), Some(remove_blocks_hook)))
    }

    fn customize_pipeline_stages<Provider>(
        &self,
        config: &reth_ethereum::node::core::node_config::NodeConfig<
            <<Node as FullNodeTypes>::Types as NodeTypes>::ChainSpec,
        >,
        stages: StageSetBuilder<Provider>,
    ) -> eyre::Result<StageSetBuilder<Provider>>
    where
        Provider: HeaderProvider<
                Header = <<Node::Types as NodeTypes>::Primitives as NodePrimitives>::BlockHeader,
            > + AccountReader
            + ChangeSetReader
            + StorageChangeSetReader
            + StorageReader
            + StateReader
            + BlockNumReader
            + Send
            + 'static,
    {
        let qmdb = self.qmdb.open_initialized(config)?;
        Ok(stages
            .disable_all(&[
                StageId::MerkleUnwind,
                StageId::AccountHashing,
                StageId::StorageHashing,
                StageId::MerkleExecute,
            ])
            .add_after(QmdbStage::new(qmdb), StageId::Execution))
    }
}

fn main() {
    let qmdb = QmdbStateLoader::default();
    let testing_qmdb = qmdb.clone();

    Cli::parse_args()
        .run(async move |builder, _| {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(
                    EthereumNode::components().payload(QmdbPayloadBuilder::new(qmdb.clone())),
                )
                .with_add_ons(
                    EthereumAddOns::default()
                        .with_testing_state_provider(Arc::new(move |provider| {
                            let qmdb = testing_qmdb.state.get().cloned().ok_or_else(|| {
                                ProviderError::other(std::io::Error::other(
                                    "QMDb state is not initialized",
                                ))
                            })?;
                            Ok(Box::new(QmdbStateRootProvider::new(provider, qmdb))
                                as StateProviderBox)
                        }))
                        .with_engine_validator(QmdbEngineValidatorBuilder::new(
                            EthereumEngineValidatorBuilder::default(),
                            qmdb,
                        )),
                )
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256};
    use alloy_rpc_types_engine::{
        CancunPayloadFields, ExecutionData, ExecutionPayload as RpcExecutionPayload,
        ExecutionPayloadSidecar, ForkchoiceState, PayloadAttributes, TestingBuildBlockRequestV1,
    };
    use reth_chainspec::{ChainSpecBuilder, EthereumHardforks, MAINNET};
    use reth_e2e_test_utils::node::NodeTestContext;
    use reth_node_api::TreeConfig;
    use reth_node_builder::{
        EngineNodeLauncher, FullNodeComponents, NodeBuilder, NodeHandle, NodeTypes,
    };
    use reth_node_core::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
    use reth_provider::{providers::BlockchainProvider, BlockNumReader, HeaderProvider};
    use reth_rpc_server_types::RpcModuleSelection;
    use reth_tasks::Runtime;
    use std::{sync::Arc, time::Duration};

    const fn test_payload_attributes(timestamp: u64) -> PayloadAttributes {
        PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
            slot_number: None,
        }
    }

    async fn run_qmdb_engine_e2e(blocks: u64, storage_v2: bool) -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let runtime = Runtime::test();
        let qmdb = QmdbStateLoader::default();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .cancun_activated()
                .build(),
        );
        let network = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..Default::default() },
            ..Default::default()
        };
        let rpc = RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_http_api(RpcModuleSelection::All);
        let mut config = reth_node_builder::NodeConfig::new(chain_spec)
            .with_network(network)
            .with_unused_ports()
            .with_rpc(rpc)
            .set_dev(true);
        config.storage.v2 = storage_v2;

        let tree_config =
            TreeConfig::default().with_persistence_threshold(0).with_memory_block_buffer_target(0);
        let testing_qmdb = qmdb.clone();
        let NodeHandle { node, node_exit_future: _node_exit_future } = NodeBuilder::new(config)
            .testing_node(runtime.clone())
            .with_types_and_provider::<EthereumNode, BlockchainProvider<_>>()
            .with_components(
                EthereumNode::components().payload(QmdbPayloadBuilder::new(qmdb.clone())),
            )
            .with_add_ons(
                EthereumAddOns::default()
                    .with_testing_state_provider(Arc::new(move |provider| {
                        let qmdb = testing_qmdb.state.get().cloned().ok_or_else(|| {
                            ProviderError::other(std::io::Error::other(
                                "QMDb state is not initialized",
                            ))
                        })?;
                        Ok(Box::new(QmdbStateRootProvider::new(provider, qmdb)) as StateProviderBox)
                    }))
                    .with_engine_validator(QmdbEngineValidatorBuilder::new(
                        EthereumEngineValidatorBuilder::default(),
                        qmdb.clone(),
                    )),
            )
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    builder.task_executor().clone(),
                    builder.config().datadir(),
                    tree_config,
                );
                builder.launch_with(launcher)
            })
            .await?;

        let mut node = NodeTestContext::new(node, test_payload_attributes).await?;
        let genesis = node.block_hash(0);
        node.update_forkchoice(genesis, genesis).await?;

        let built = node
            .testing_build_block_v1(TestingBuildBlockRequestV1 {
                parent_block_hash: genesis,
                payload_attributes: test_payload_attributes(1),
                transactions: vec![],
                extra_data: None,
            })
            .await?;
        let testing_block_hash = built.execution_payload.payload_inner.payload_inner.block_hash;
        let versioned_hashes = built.blobs_bundle.versioned_hashes();
        let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
            parent_beacon_block_root: B256::ZERO,
            versioned_hashes,
        });
        node.inner
            .add_ons_handle
            .beacon_engine_handle
            .new_payload(ExecutionData {
                payload: RpcExecutionPayload::V3(built.execution_payload),
                sidecar,
            })
            .await?;
        update_forkchoice(&node, genesis, testing_block_hash).await?;
        wait_for_head(&node, 1, testing_block_hash).await?;

        for _ in 1..blocks.saturating_sub(1) {
            node.advance_block().await?;
        }

        let parent = node.block_hash(blocks.saturating_sub(1));
        let payload_a = node.build_and_submit_payload().await?;
        let hash_a = payload_a.block().hash();
        let payload_b = node.build_and_submit_payload().await?;
        let hash_b = payload_b.block().hash();

        update_forkchoice(&node, parent, hash_a).await?;
        wait_for_head(&node, blocks, hash_a).await?;
        update_forkchoice(&node, parent, hash_b).await?;
        wait_for_head(&node, blocks, hash_b).await?;

        let qmdb_config = QmdbConfig::new(node.inner.config.datadir().data_dir().join("qmdb"))
            .with_partition_prefix("state");
        let shutdown = node
            .inner
            .add_ons_handle
            .engine_shutdown
            .shutdown()
            .expect("engine shutdown should be available");
        shutdown.await?;

        let provider = node.inner.provider.clone();
        let state = qmdb.open_for_provider(&node.inner.config, &provider)?;
        let head = state.head()?.expect("QMDb head should exist after engine persistence");
        let header = provider.header_by_number(blocks)?.expect("canonical header should exist");
        assert_eq!(provider.last_block_number()?, blocks);
        assert_eq!(head.number, blocks);
        assert_eq!(head.hash, hash_b);
        assert_eq!(head.root, header.state_root());

        drop(state);
        drop(node);
        drop(qmdb);

        let reopened = QmdbState::open(qmdb_config)?;
        assert_eq!(reopened.head()?, Some(head));
        assert_eq!(reopened.root()?, head.root);

        Ok(())
    }

    async fn update_forkchoice<Node, AddOns>(
        node: &NodeTestContext<Node, AddOns>,
        finalized: B256,
        head: B256,
    ) -> eyre::Result<()>
    where
        Node: FullNodeComponents,
        AddOns: reth_node_builder::rpc::RethRpcAddOns<Node>,
    {
        node.inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(
                ForkchoiceState {
                    head_block_hash: head,
                    safe_block_hash: finalized,
                    finalized_block_hash: finalized,
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn wait_for_head<Node, AddOns>(
        node: &NodeTestContext<Node, AddOns>,
        number: u64,
        hash: B256,
    ) -> eyre::Result<()>
    where
        Node: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
        AddOns: reth_node_builder::rpc::RethRpcAddOns<Node>,
    {
        for _ in 0..500 {
            if node.inner.provider.last_block_number()? == number && node.block_hash(number) == hash
            {
                return Ok(())
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        eyre::bail!("timed out waiting for canonical head {number} {hash}")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn qmdb_engine_e2e_small() -> eyre::Result<()> {
        run_qmdb_engine_e2e(5, false).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn qmdb_engine_e2e_storage_v2_small() -> eyre::Result<()> {
        run_qmdb_engine_e2e(5, true).await
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "QMDb engine e2e scale check; run explicitly for timing"]
    async fn qmdb_engine_e2e_10() -> eyre::Result<()> {
        run_qmdb_engine_e2e(10, true).await
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "QMDb engine e2e scale check; run explicitly for timing"]
    async fn qmdb_engine_e2e_100() -> eyre::Result<()> {
        run_qmdb_engine_e2e(100, true).await
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "QMDb engine e2e scale check; run explicitly for timing"]
    async fn qmdb_engine_e2e_1000() -> eyre::Result<()> {
        run_qmdb_engine_e2e(1_000, true).await
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "QMDb engine e2e scale check; run explicitly for timing"]
    async fn qmdb_engine_e2e_10k() -> eyre::Result<()> {
        run_qmdb_engine_e2e(10_000, true).await
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "100k QMDb engine e2e; run explicitly for long-chain validation"]
    async fn qmdb_engine_e2e_100k() -> eyre::Result<()> {
        run_qmdb_engine_e2e(100_000, true).await
    }
}
