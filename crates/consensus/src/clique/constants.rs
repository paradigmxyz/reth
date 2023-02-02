//! Clique constants.
//! See https://eips.ethereum.org/EIPS/eip-225 for more information.

use std::time::Duration;

/// Minimum difference between two consecutive block’s timestamps.
pub(crate) const BLOCK_PERIOD: Duration = Duration::from_secs(15);

/// Number of blocks after which to checkpoint and reset the pending votes.
pub(crate) const EPOCH_LENGTH: u64 = 30000;

/// Fixed number of extra-data prefix bytes reserved for signer vanity.
pub(crate) const EXTRA_VANITY: usize = 32;

/// Fixed number of extra-data suffix bytes reserved for signer signature.
/// 65 bytes fixed as signatures are based on the standard secp256k1 curve.
/// Filled with zeros on genesis block.
pub(crate) const EXTRA_SEAL: usize = 65;

/// Magic nonce number `0xffffffffffffffff` to vote on adding a new signer.
pub(crate) const NONCE_AUTH_VOTE: [u8; 8] = [0xff; 8];

/// Magic nonce number `0x0000000000000000` to vote on removing a signer.
pub(crate) const NONCE_DROP_VOTE: [u8; 8] = [0x00; 8];

/// Block score (difficulty) for blocks containing out-of-turn signatures.
/// Suggested 1 since it just needs to be an arbitrary baseline constant.
pub(crate) const DIFF_NOTURN: u64 = 1;

/// Block score (difficulty) for blocks containing in-turn signatures.
/// Suggested 2 to show a slight preference over out-of-turn signatures.
pub(crate) const DIFF_INTURN: u64 = 2;
