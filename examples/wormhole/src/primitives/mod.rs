pub mod block;
pub mod engine;
pub mod tx;

pub use block::{
    WormholeBlock, WormholeBlockBody, WormholeBlockWithSenders, WormholeReceipt,
    WormholeSealedBlock, WormholeSealedBlockWithSenders,
};
pub use engine::{WormholeEngineTypes, WormholePayloadTypes};
pub use tx::{WormholeTransaction, WormholeTransactionSigned, WORMHOLE_TX_TYPE_ID};

use alloy_consensus::Header;

/// Wormhole node primitives
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WormholeNodePrimitives;

impl reth_node_api::NodePrimitives for WormholeNodePrimitives {
    type Block = WormholeBlock;
    type BlockHeader = Header;
    type BlockBody = WormholeBlockBody;
    type SignedTx = WormholeTransactionSigned;
    type Receipt = WormholeReceipt;
}
