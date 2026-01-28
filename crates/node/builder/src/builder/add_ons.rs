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
    pub add_ons: AddOns,
    /// The threshold for the number of blocks in the WAL before emitting a warning.
    ///
    /// For L2 chains with faster block times, this value should be increased proportionally
    /// to avoid excessive warnings. For example, a chain with 2-second block times might use
    /// a value 6x higher than the default (768 instead of 128).
    pub wal_blocks_warning: Option<usize>,
}

impl<Node: FullNodeComponents, AO: NodeAddOns<Node>> AddOns<Node, AO> {
    /// Sets the threshold for the number of blocks in the WAL before emitting a warning.
    ///
    /// For L2 chains with faster block times, this value should be increased proportionally
    /// to avoid excessive warnings. For example, a chain with 2-second block times might use
    /// a value 6x higher than the default (768 instead of 128).
    pub const fn with_wal_blocks_warning(mut self, threshold: usize) -> Self {
        self.wal_blocks_warning = Some(threshold);
        self
    }
}
