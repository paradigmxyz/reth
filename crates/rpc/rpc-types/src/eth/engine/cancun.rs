//! Contains types related to the Cancun hardfork that will be used by RPC to communicate with the
//! beacon consensus engine.

use alloy_primitives::B256;

/// Fields introduced in `engine_newPayloadV3` that are not present in the `ExecutionPayload` RPC
/// object.
///
/// See also:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#request>
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CancunPayloadFields {
    /// The parent beacon block root.
    pub parent_beacon_block_root: B256,

    /// The expected blob versioned hashes.
    pub versioned_hashes: Vec<B256>,
}

/// A container type for [CancunPayloadFields] that may or may not be present.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct MaybeCancunPayloadFields {
    fields: Option<CancunPayloadFields>,
}

impl MaybeCancunPayloadFields {
    /// Returns a new `MaybeCancunPayloadFields` with no cancun fields.
    pub const fn none() -> Self {
        Self { fields: None }
    }

    /// Returns a new `MaybeCancunPayloadFields` with the given cancun fields.
    pub fn into_inner(self) -> Option<CancunPayloadFields> {
        self.fields
    }

    /// Returns the parent beacon block root, if any.
    pub fn parent_beacon_block_root(&self) -> Option<B256> {
        self.fields.as_ref().map(|fields| fields.parent_beacon_block_root)
    }

    /// Returns the blob versioned hashes, if any.
    pub fn versioned_hashes(&self) -> Option<&Vec<B256>> {
        self.fields.as_ref().map(|fields| &fields.versioned_hashes)
    }
}

impl From<CancunPayloadFields> for MaybeCancunPayloadFields {
    #[inline]
    fn from(fields: CancunPayloadFields) -> Self {
        Self { fields: Some(fields) }
    }
}

impl From<Option<CancunPayloadFields>> for MaybeCancunPayloadFields {
    #[inline]
    fn from(fields: Option<CancunPayloadFields>) -> Self {
        Self { fields }
    }
}
