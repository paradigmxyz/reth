use crate::Address;
use reth_codecs::{main_codec, Compact};
use reth_rlp::{RlpDecodable, RlpEncodable};

/// Withdrawal represents a validator withdrawal from the consensus layer.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct Withdrawal {
    /// Monotonically increasing identifier issued by consensus layer.
    pub index: u64,
    /// Index of validator associated with withdrawal.
    pub validator_index: u64,
    /// Target address for withdrawn ether.
    pub address: Address,
    /// Value of the withdrawal in gwei.
    pub amount: u64,
}
