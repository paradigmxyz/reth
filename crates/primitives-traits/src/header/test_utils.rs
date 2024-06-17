//! Test utilities to generate random valid headers.

use crate::Header;
use alloy_primitives::B256;
use proptest::{arbitrary::any, prop_compose};

/// Generates a header which is valid __with respect to past and future forks__. This means, for
/// example, that if the withdrawals root is present, the base fee per gas is also present.
///
/// If blob gas used were present, then the excess blob gas and parent beacon block root are also
/// present. In this example, the withdrawals root would also be present.
///
/// This __does not, and should not guarantee__ that the header is valid with respect to __anything
/// else__.
pub const fn generate_valid_header(
    mut header: Header,
    eip_4844_active: bool,
    blob_gas_used: u64,
    excess_blob_gas: u64,
    parent_beacon_block_root: B256,
) -> Header {
    // EIP-1559 logic
    if header.base_fee_per_gas.is_none() {
        // If EIP-1559 is not active, clear related fields
        header.withdrawals_root = None;
        header.blob_gas_used = None;
        header.excess_blob_gas = None;
        header.parent_beacon_block_root = None;
    } else if header.withdrawals_root.is_none() {
        // If EIP-4895 is not active, clear related fields
        header.blob_gas_used = None;
        header.excess_blob_gas = None;
        header.parent_beacon_block_root = None;
    } else if eip_4844_active {
        // Set fields based on EIP-4844 being active
        header.blob_gas_used = Some(blob_gas_used);
        header.excess_blob_gas = Some(excess_blob_gas);
        header.parent_beacon_block_root = Some(parent_beacon_block_root);
    } else {
        // If EIP-4844 is not active, clear related fields
        header.blob_gas_used = None;
        header.excess_blob_gas = None;
        header.parent_beacon_block_root = None;
    }

    // todo(onbjerg): adjust this for eip-7589
    header.requests_root = None;

    header
}

prop_compose! {
    /// Generates a proptest strategy for constructing an instance of a header which is valid __with
    /// respect to past and future forks__.
    ///
    /// See docs for [generate_valid_header] for more information.
    pub fn valid_header_strategy()(
        header in any::<Header>(),
        eip_4844_active in any::<bool>(),
        blob_gas_used in any::<u64>(),
        excess_blob_gas in any::<u64>(),
        parent_beacon_block_root in any::<B256>()
    ) -> Header {
        generate_valid_header(header, eip_4844_active, blob_gas_used, excess_blob_gas, parent_beacon_block_root)
    }
}
