//! Types for EIP-7928 block access lists produced by block execution.

use alloc::sync::Arc;

/// A decoded EIP-7928 block access list in the representation evm2 consumes, paired with the
/// raw RLP payload it was decoded from.
///
/// Produced once during block validation and shared with consumers such as the RPC state
/// cache, so the RLP payload does not have to be decoded and converted a second time.
pub type DecodedEvmBal = alloy_eip7928::bal::DecodedBal<Arc<evm2::evm::Bal>>;
