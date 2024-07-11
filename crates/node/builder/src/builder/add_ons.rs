//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use std::sync::Arc;

use auto_impl::auto_impl;
use reth_node_api::{FullNodeComponents, NodeAddOns};

use crate::{
    exex::BoxedLaunchExEx,
    hooks::NodeHooks,
    rpc::{EthApiBuilder, RpcHooks},
};

/// Additional node extensions.
///
/// At this point we consider all necessary components defined.
pub struct AddOns<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub hooks: NodeHooks<Node, AddOns>,
    /// The `ExExs` (execution extensions) of the node.
    pub exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    /// Additional RPC add ons.
    pub rpc: RpcAddOns<Node, AddOns::EthApi>,
}

/// Additional context addon that captures settings required for a regular, ethereum or optimism
/// node that make use of engine API and RPC.
pub struct EngineApiAddon<Node: FullNodeComponents, EthApi: Send> {
    rpc: RpcAddOns<Node, EthApi>,
    // TODO anything rpc specific we need here? if not, maybe this type is redundant and engine can
    // be enforced via the launcher type entirely
}

/// Captures node specific addons that can be installed on top of the type configured node and are
/// required for launching the node, such as RPC.
pub struct RpcAddOns<Node: FullNodeComponents, EthApi: Send> {
    // TODO enforce the the ethAPI builder trait
    pub eth_api_builder: Arc<dyn EthApiBuilder<Node, Output = EthApi>>,
    /// Additional RPC hooks.
    pub hooks: RpcHooks<Node, EthApi>,
}

/// Aggregates builders for the customizable node add-ons.
#[auto_impl(Arc)]
pub trait NodeAddOnBuilders<N: FullNodeComponents, AO: NodeAddOns<N>>:
    Send + Sync + Unpin + 'static
{
    /// Returns an [`EthApiBuilder`] for the node.
    fn eth_api_builder(&self) -> Arc<dyn EthApiBuilder<N, Output = AO::EthApi>>;
}

impl<N: FullNodeComponents> NodeAddOnBuilders<N, ()> for Option<()> {
    fn eth_api_builder(&self) -> Arc<dyn EthApiBuilder<N, Output = ()>> {
        Arc::new(None)
    }
}

/// Adapter containing the customizable node add-on builders.
pub struct AddOnBuildersAdapter<N: FullNodeComponents, AO: NodeAddOns<N>> {
    pub eth_api_builder: Arc<dyn EthApiBuilder<N, Output = AO::EthApi>>,
}

impl<N, AO> NodeAddOnBuilders<N, AO> for AddOnBuildersAdapter<N, AO>
where
    N: FullNodeComponents,
    AO: NodeAddOns<N>,
{
    fn eth_api_builder(&self) -> Arc<dyn EthApiBuilder<N, Output = AO::EthApi>> {
        self.eth_api_builder.clone()
    }
}
