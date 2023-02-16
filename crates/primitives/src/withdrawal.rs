use crate::{constants::GWEI_TO_WEI, Address, U256};
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

impl Withdrawal {
    /// Return the withdrawal amount in wei.
    pub fn amount_wei(&self) -> U256 {
        U256::from(self.amount) * U256::from(GWEI_TO_WEI)
    }
}
