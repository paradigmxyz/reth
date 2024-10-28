/// Configures all the primitive types of the node.
pub trait NodePrimitives {
    /// Block primitive.
    type Block;
}

impl NodePrimitives for () {
    type Block = ();
}
