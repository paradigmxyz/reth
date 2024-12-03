//! A generic [`NodeComponentsBuilder`]

use crate::{
    components::{
        Components, ConsensusBuilder, ExecutorBuilder, NetworkBuilder, NodeComponents,
        PayloadServiceBuilder, PoolBuilder,
    },
    BuilderContext, ConfigureEvm, FullNodeTypes,
};
use alloy_consensus::Header;
use reth_consensus::FullConsensus;
use reth_evm::execute::BlockExecutorProvider;
use reth_node_api::{NodeTypes, NodeTypesWithEngine, TxTy};
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{future::Future, marker::PhantomData};

/// A generic, general purpose and customizable [`NodeComponentsBuilder`] implementation.
///
/// This type is stateful and captures the configuration of the node's components.
///
/// ## Component dependencies:
///
/// The components of the node depend on each other:
/// - The payload builder service depends on the transaction pool.
/// - The network depends on the transaction pool.
///
/// We distinguish between different kind of components:
/// - Components that are standalone, such as the transaction pool.
/// - Components that are spawned as a service, such as the payload builder service or the network.
///
/// ## Builder lifecycle:
///
/// First all standalone components are built. Then the service components are spawned.
/// All component builders are captured in the builder state and will be consumed once the node is
/// launched.
#[derive(Debug)]
pub struct ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB> {
    pool_builder: PoolB,
    payload_builder: PayloadB,
    network_builder: NetworkB,
    executor_builder: ExecB,
    consensus_builder: ConsB,
    _marker: PhantomData<Node>,
}

impl<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
    ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
{
    /// Configures the node types.
    pub fn node_types<Types>(
        self,
    ) -> ComponentsBuilder<Types, PoolB, PayloadB, NetworkB, ExecB, ConsB>
    where
        Types: FullNodeTypes,
    {
        let Self {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        } = self;
        ComponentsBuilder {
            executor_builder: evm_builder,
            pool_builder,
            payload_builder,
            network_builder,
            consensus_builder,
            _marker: Default::default(),
        }
    }

    /// Apply a function to the pool builder.
    pub fn map_pool(self, f: impl FnOnce(PoolB) -> PoolB) -> Self {
        Self {
            pool_builder: f(self.pool_builder),
            payload_builder: self.payload_builder,
            network_builder: self.network_builder,
            executor_builder: self.executor_builder,
            consensus_builder: self.consensus_builder,
            _marker: self._marker,
        }
    }

    /// Apply a function to the payload builder.
    pub fn map_payload(self, f: impl FnOnce(PayloadB) -> PayloadB) -> Self {
        Self {
            pool_builder: self.pool_builder,
            payload_builder: f(self.payload_builder),
            network_builder: self.network_builder,
            executor_builder: self.executor_builder,
            consensus_builder: self.consensus_builder,
            _marker: self._marker,
        }
    }

    /// Apply a function to the network builder.
    pub fn map_network(self, f: impl FnOnce(NetworkB) -> NetworkB) -> Self {
        Self {
            pool_builder: self.pool_builder,
            payload_builder: self.payload_builder,
            network_builder: f(self.network_builder),
            executor_builder: self.executor_builder,
            consensus_builder: self.consensus_builder,
            _marker: self._marker,
        }
    }

    /// Apply a function to the executor builder.
    pub fn map_executor(self, f: impl FnOnce(ExecB) -> ExecB) -> Self {
        Self {
            pool_builder: self.pool_builder,
            payload_builder: self.payload_builder,
            network_builder: self.network_builder,
            executor_builder: f(self.executor_builder),
            consensus_builder: self.consensus_builder,
            _marker: self._marker,
        }
    }

    /// Apply a function to the consensus builder.
    pub fn map_consensus(self, f: impl FnOnce(ConsB) -> ConsB) -> Self {
        Self {
            pool_builder: self.pool_builder,
            payload_builder: self.payload_builder,
            network_builder: self.network_builder,
            executor_builder: self.executor_builder,
            consensus_builder: f(self.consensus_builder),
            _marker: self._marker,
        }
    }
}

impl<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
    ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
where
    Node: FullNodeTypes,
{
    /// Configures the pool builder.
    ///
    /// This accepts a [`PoolBuilder`] instance that will be used to create the node's transaction
    /// pool.
    pub fn pool<PB>(
        self,
        pool_builder: PB,
    ) -> ComponentsBuilder<Node, PB, PayloadB, NetworkB, ExecB, ConsB>
    where
        PB: PoolBuilder<Node>,
    {
        let Self {
            pool_builder: _,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        }
    }
}

impl<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
    ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder<Node>,
{
    /// Configures the network builder.
    ///
    /// This accepts a [`NetworkBuilder`] instance that will be used to create the node's network
    /// stack.
    pub fn network<NB>(
        self,
        network_builder: NB,
    ) -> ComponentsBuilder<Node, PoolB, PayloadB, NB, ExecB, ConsB>
    where
        NB: NetworkBuilder<Node, PoolB::Pool>,
    {
        let Self {
            pool_builder,
            payload_builder,
            network_builder: _,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        }
    }

    /// Configures the payload builder.
    ///
    /// This accepts a [`PayloadServiceBuilder`] instance that will be used to create the node's
    /// payload builder service.
    pub fn payload<PB>(
        self,
        payload_builder: PB,
    ) -> ComponentsBuilder<Node, PoolB, PB, NetworkB, ExecB, ConsB>
    where
        PB: PayloadServiceBuilder<Node, PoolB::Pool>,
    {
        let Self {
            pool_builder,
            payload_builder: _,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        }
    }

    /// Configures the executor builder.
    ///
    /// This accepts a [`ExecutorBuilder`] instance that will be used to create the node's
    /// components for execution.
    pub fn executor<EB>(
        self,
        executor_builder: EB,
    ) -> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, EB, ConsB>
    where
        EB: ExecutorBuilder<Node>,
    {
        let Self {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: _,
            consensus_builder,
            _marker,
        } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder,
            consensus_builder,
            _marker,
        }
    }

    /// Configures the consensus builder.
    ///
    /// This accepts a [`ConsensusBuilder`] instance that will be used to create the node's
    /// components for consensus.
    pub fn consensus<CB>(
        self,
        consensus_builder: CB,
    ) -> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, CB>
    where
        CB: ConsensusBuilder<Node>,
    {
        let Self {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder,
            consensus_builder: _,

            _marker,
        } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder,
            consensus_builder,
            _marker,
        }
    }
}

impl<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB> NodeComponentsBuilder<Node>
    for ComponentsBuilder<Node, PoolB, PayloadB, NetworkB, ExecB, ConsB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder<Node>,
    NetworkB: NetworkBuilder<Node, PoolB::Pool>,
    PayloadB: PayloadServiceBuilder<Node, PoolB::Pool>,
    ExecB: ExecutorBuilder<Node>,
    ConsB: ConsensusBuilder<Node>,
{
    type Components = Components<Node, PoolB::Pool, ExecB::EVM, ExecB::Executor, ConsB::Consensus>;

    async fn build_components(
        self,
        context: &BuilderContext<Node>,
    ) -> eyre::Result<Self::Components> {
        let Self {
            pool_builder,
            payload_builder,
            network_builder,
            executor_builder: evm_builder,
            consensus_builder,
            _marker,
        } = self;

        let (evm_config, executor) = evm_builder.build_evm(context).await?;
        let pool = pool_builder.build_pool(context).await?;
        let network = network_builder.build_network(context, pool.clone()).await?;
        let payload_builder = payload_builder.spawn_payload_service(context, pool.clone()).await?;
        let consensus = consensus_builder.build_consensus(context).await?;

        Ok(Components {
            transaction_pool: pool,
            evm_config,
            network,
            payload_builder,
            executor,
            consensus,
        })
    }
}

impl Default for ComponentsBuilder<(), (), (), (), (), ()> {
    fn default() -> Self {
        Self {
            pool_builder: (),
            payload_builder: (),
            network_builder: (),
            executor_builder: (),
            consensus_builder: (),
            _marker: Default::default(),
        }
    }
}

/// A type that configures all the customizable components of the node and knows how to build them.
///
/// Implementers of this trait are responsible for building all the components of the node: See
/// [`NodeComponents`].
///
/// The [`ComponentsBuilder`] is a generic, general purpose implementation of this trait that can be
/// used to customize certain components of the node using the builder pattern and defaults, e.g.
/// Ethereum and Optimism.
/// A type that's responsible for building the components of the node.
pub trait NodeComponentsBuilder<Node: FullNodeTypes>: Send {
    /// The components for the node with the given types
    type Components: NodeComponents<
        Node,
        PayloadBuilder = PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine>,
    >;

    /// Consumes the type and returns the created components.
    fn build_components(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Components>> + Send;
}

impl<Node, F, Fut, Pool, EVM, Executor, Cons> NodeComponentsBuilder<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Components<Node, Pool, EVM, Executor, Cons>>> + Send,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    EVM: ConfigureEvm<Header = Header>,
    Executor: BlockExecutorProvider<Primitives = <Node::Types as NodeTypes>::Primitives>,
    Cons: FullConsensus<<Node::Types as NodeTypes>::Primitives> + Clone + Unpin + 'static,
{
    type Components = Components<Node, Pool, EVM, Executor, Cons>;

    fn build_components(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Components>> + Send {
        self(ctx)
    }
}
