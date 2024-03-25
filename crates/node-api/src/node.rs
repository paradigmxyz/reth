//! Traits for configuring a node

use crate::{primitives::NodePrimitives, ConfigureEvm, EngineTypes};

/// The type that configures the essential types of an ethereum like node.
///
/// This includes the primitive types of a node, the engine API types for communication with the
/// consensus layer, and the EVM configuration type for setting up the Ethereum Virtual Machine.
pub trait NodeTypes: Send + Sync + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm;

    /// Returns the node's evm config.
    fn evm_config(&self) -> Self::Evm;
}
