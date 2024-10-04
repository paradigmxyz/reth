//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use reth_node_api::{FullNodeComponents, NodeAddOns};

use crate::{exex::BoxedLaunchExEx, hooks::NodeHooks};

/// Additional node extensions.
///
/// At this point we consider all necessary components defined.
pub struct AddOns<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub hooks: NodeHooks<Node, AddOns>,
    /// The `ExExs` (execution extensions) of the node.
    pub exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    /// Additional captured addons.
    pub addons: AddOns,
}
