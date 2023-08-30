//! Contains types related to the Cancun hardfork that will be used by RPC to communicate with the
//! beacon consensus engine.
use reth_primitives::H256;

/// Fields introduced in `engine_newPayloadV3` that are not present in the `ExecutionPayload` RPC
/// object.
///
/// See also:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#request>
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CancunPayloadFields {
    /// The parent beacon block root.
    pub parent_beacon_block_root: H256,

    /// The expected blob versioned hashes.
    pub versioned_hashes: Vec<H256>,
}
