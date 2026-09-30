//! Types for EIP-7928 block access lists produced by block execution.

use alloc::sync::Arc;

/// A decoded EIP-7928 block access list in the representation revm consumes, paired with the
/// raw RLP payload it was decoded from.
///
/// Produced once during block validation and shared with consumers such as the RPC state
/// cache, so the RLP payload does not have to be decoded and converted a second time.
pub type DecodedRevmBal = alloy_eip7928::bal::DecodedBal<Arc<revm::state::bal::Bal>>;
