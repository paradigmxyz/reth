use reth_node_types::NodePrimitives;

use crate::providers::StaticFileProvider;

/// Static file provider factory.
pub trait StaticFileProviderFactory {
    /// The network primitives type [`StaticFileProvider`] is using.
    type Primitives: NodePrimitives;

    /// Create new instance of static file provider.
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives>;
}
