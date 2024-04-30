//! Keys of ENR [`ForkId`](reth_primitives::ForkId) kv-pair. Identifies which network a node
//! belongs to.

/// ENR fork ID kv-pair key, for an Ethereum L1 EL node.
pub const ETH: &[u8] = b"eth";

/// ENR fork ID kv-pair key, for an Ethereum L1 CL node.
pub const ETH2: &[u8] = b"eth2";

/// ENR fork ID kv-pair key, for an Optimism EL node.
pub const OPEL: &[u8] = b"opel";

/// ENR fork ID kv-pair key, for an Optimism CL node.
pub const OPSTACK: &[u8] = b"opstack";
