//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use crate::{exex::BoxedLaunchExEx, hooks::NodeHooks, stages::BoxedInstallStages};
use reth_node_api::{FullNodeComponents, NodeAddOns};

/// Additional node extensions.
///
/// At this point we consider all necessary components defined.
pub struct AddOns<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub hooks: NodeHooks<Node, AddOns>,
    /// The `ExExs` (execution extensions) of the node.
    pub exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    /// Custom stage installers
    pub stage_installer: Option<Box<dyn BoxedInstallStages<Node>>>,
    /// Additional captured addons.
    pub add_ons: AddOns,
}
