use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Encodable};
use reth_ress_protocol::ExecutionWitness;

/// A trait bound for zk-ress witnesses.
pub trait ZkRessWitness: Encodable + Decodable + Send + Sync {}

/// Blanket witness implementation for arbitrary bytes.
impl ZkRessWitness for Bytes {}

impl ZkRessWitness for ExecutionWitness {}
