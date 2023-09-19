//! Eth ABI helpers.

/// Returns the revert reason from the given output data, if it's an abi encoded String. Returns
/// `None` if the output is not long enough to contain a function selector or the content is not a
/// valid abi encoded String.
///
/// **Note:** it's assumed the `out` buffer starts with the call's signature
pub fn decode_revert_reason(_out: impl AsRef<[u8]>) -> Option<String> {
    todo!("use alloy_sol_types")
}
