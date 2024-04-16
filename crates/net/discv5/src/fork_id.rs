//! ENR keys used to identify networks Alternative fork ID formats. For Ethereum EL see
//! [`ForkId`](reth_primitives::ForkId).

use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_primitives::ChainId;

/// ENR fork ID kv-pair key, for an Ethereum L1 EL node.
pub const ENR_FORK_KEY_ETH: &[u8] = b"eth";

/// ENR fork ID kv-pair key, for an Ethereum L1 CL node.
pub const ENR_FORK_KEY_ETH2: &[u8] = b"eth2";

/// ENR fork ID kv-pair key, for an Optimism node.
pub const ENR_FORK_KEY_OPSTACK: &[u8] = b"opstack";

/// Optimism fork ID, used to identify OP chains on discovery network.
///
/// <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/rollup-node-p2p.md#structure>
#[derive(RlpEncodable, Debug, PartialEq, Eq, RlpDecodable)]
pub struct OptimismForkId {
    chain_id: ChainId,
    /// Note: in practice, fork ID isn't used, it's set to zero.
    _fork_id: usize,
}

impl OptimismForkId {
    /// Returns a new instance for the given chain.
    pub const fn new(chain_id: ChainId) -> Self {
        Self { chain_id, _fork_id: 0 }
    }
}
