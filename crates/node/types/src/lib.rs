//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_chainspec::EthChainSpec;
use reth_db_api::{
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    Database,
};
use reth_engine_primitives::EngineTypes;

/// Configures all the primitive types of the node.
// TODO(mattsse): this is currently a placeholder
pub trait NodePrimitives {}

// TODO(mattsse): Placeholder
impl NodePrimitives for () {}

/// The type that configures the essential types of an Ethereum-like node.
///
/// This includes the primitive types of a node and chain specification.
///
/// This trait is intended to be stateless and only define the types of the node.
pub trait NodeTypes: Send + Sync + Unpin + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The type used for configuration of the EVM.
    type ChainSpec: EthChainSpec;
}

/// The type that configures an Ethereum-like node with an engine for consensus.
pub trait NodeTypesWithEngine: NodeTypes {
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
}

/// A helper trait that is downstream of the [`NodeTypesWithEngine`] trait and adds database to the
/// node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait NodeTypesWithDB: NodeTypesWithEngine {
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static;
}
