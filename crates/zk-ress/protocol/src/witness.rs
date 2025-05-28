use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};

/// A trait bound for zk-ress witnesses.
pub trait ZkRessWitness: Encodable + Decodable + Send + Sync {}

/// Full execution witness with trie nodes and bytecodes accessed in the block.
#[derive(RlpEncodable, RlpDecodable, PartialEq, Eq, Clone, Default, Debug)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct ExecutionWitness {
    /// State trie nodes.
    pub state: Vec<Bytes>,
    /// Bytecodes.
    pub bytecodes: Vec<Bytes>,
}

impl ZkRessWitness for ExecutionWitness {}
