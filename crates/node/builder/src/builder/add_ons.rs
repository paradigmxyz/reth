//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use reth_db_api::database::Database;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeAddOns, NodeTypesWithDBAdapter};
use reth_provider::DatabaseProvider;
use reth_prune::segments::Segment;
use std::sync::Arc;

use crate::{exex::BoxedLaunchExEx, hooks::NodeHooks};

/// Additional node extensions.
///
/// At this point we consider all necessary components defined.
pub struct AddOns<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub hooks: NodeHooks<Node, AddOns>,
    /// The `ExExs` (execution extensions) of the node.
    pub exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    /// Additional prune segments that are run by the node's pruners, see
    /// [`PruneSegment::Custom`](reth_prune::PruneSegment::Custom).
    ///
    /// Segments are `Arc`ed because each instance is shared between the engine-driven pruner and
    /// the pipeline's prune stage.
    pub prune_segments: Vec<Arc<dyn Segment<PrunerProviderRW<Node>>>>,
    /// Additional captured addons.
    pub add_ons: AddOns,
}

/// The read-write database provider type the node's pruner operates on, for the given
/// [`FullNodeTypes`].
///
/// This is the provider type that custom [`Segment`] implementations installed via
/// [`NodeBuilderWithComponents::install_prune_segment`](crate::NodeBuilderWithComponents::install_prune_segment)
/// are invoked with.
pub type PrunerProviderRW<N> = DatabaseProvider<
    <<N as FullNodeTypes>::DB as Database>::TXMut,
    NodeTypesWithDBAdapter<<N as FullNodeTypes>::Types, <N as FullNodeTypes>::DB>,
>;
