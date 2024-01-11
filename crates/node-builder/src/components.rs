use reth_node_api::node::FullNodeTypes;
use std::marker::PhantomData;

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct FullComponents<Types: FullNodeTypes> {
    _marker: PhantomData<Types>,
}

impl<Types: FullNodeTypes> FullComponents<Types> {}
