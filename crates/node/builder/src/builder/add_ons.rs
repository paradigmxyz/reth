//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use std::marker::PhantomData;

use reth_node_api::{FullNodeComponents, NodeAddOns};

use crate::{exex::BoxedLaunchExEx, hooks::NodeHooks, rpc::RpcHooks};

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
pub struct EngineApiAddon<Node: FullNodeComponents, EthApi> {
    rpc: RpcAddOns<Node, EthApi>,
    // TODO anything rpc specific we need here? if not, maybe this type is redundant and engine can
    // be enforced via the launcher type entirely
}

/// Captures node specific addons that can be installed on top of the type configured node and are
/// required for launching the node, such as RPC.
#[derive(Default)]
pub struct RpcAddOns<Node: FullNodeComponents, EthApi> {
    // TODO enforce the the ethAPI builder trait
    pub _eth_api: PhantomData<EthApi>,
    /// Additional RPC hooks.
    pub hooks: RpcHooks<Node, EthApi>,
}
