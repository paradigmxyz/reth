//! Eth ABI helpers.
use crate::constants::SELECTOR_LEN;

/// Returns the revert reason from the given output data, if it's an abi encoded String. Returns
/// `None` if the output is not long enough to contain a function selector or the content is not a
/// valid abi encoded String.
///
/// **Note:** it's assumed the `out` buffer starts with the call's signature
pub fn decode_revert_reason(out: impl AsRef<[u8]>) -> Option<String> {
    use ethers_core::abi::AbiDecode;
    let out = out.as_ref();
    if out.len() < SELECTOR_LEN {
        return None
    }
    String::decode(&out[SELECTOR_LEN..]).ok()
}
