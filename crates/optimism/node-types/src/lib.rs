//! EVM config for vanilla optimism.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

pub mod engine;
pub use engine::OpEngineTypes;

use reth_node_api::NodeTypes;
pub use reth_optimism_chainspec::*;
pub use reth_optimism_primitives::*;
pub use reth_trie_db::MerklePatriciaTrie;

use reth_storage_api::EthStorage;

/// ZST that aggregates the Optimism [`NodeTypes`].
#[derive(Clone, Debug)]
pub struct OpTypes;

impl NodeTypes for OpTypes {
    type Primitives = OpPrimitives;
    type ChainSpec = OpChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = OpStorage;
    type Payload = OpEngineTypes;
}

/// Storage implementation for Optimism.
pub type OpStorage = EthStorage<OpTransactionSigned>;
