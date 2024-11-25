use alloy_primitives::{map::HashMap, B256};

/// A Keccak code hash.
type KeccakHash = B256;
/// A Poseidon code hash.
type PoseidonHash = B256;
/// Size of a contract's code in bytes.
type CodeSize = u64;

/// Scroll post execution context maps a Keccak code hash of a contract's bytecode to its code size
/// and Poseidon code hash.
pub type ScrollPostExecutionContext = HashMap<KeccakHash, (CodeSize, PoseidonHash)>;
