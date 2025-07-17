//! Consensus types for Era post-merge history files

/// Compressed signed beacon block
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedsignedbeaconblock>.
#[derive(Debug, Clone)]
pub struct CompressedSignedBeaconBlock {
    /// Snappy-compressed ssz-encoded SignedBeaconBlock
    pub data: Vec<u8>,
}

/// Compressed beacon state
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#compressedbeaconstate>.
#[derive(Debug, Clone)]
pub struct CompressedBeaconState {
    /// Snappy-compressed ssz-encoded BeaconState
    pub data: Vec<u8>,
}