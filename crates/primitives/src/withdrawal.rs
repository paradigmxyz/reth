use crate::{constants::GWEI_TO_WEI, serde_helper::u64_hex, Address, U256};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{RlpDecodable, RlpEncodable};

/// Withdrawal represents a validator withdrawal from the consensus layer.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct Withdrawal {
    /// Monotonically increasing identifier issued by consensus layer.
    #[serde(with = "u64_hex")]
    pub index: u64,
    /// Index of validator associated with withdrawal.
    #[serde(with = "u64_hex", rename = "validatorIndex")]
    pub validator_index: u64,
    /// Target address for withdrawn ether.
    pub address: Address,
    /// Value of the withdrawal in gwei.
    #[serde(with = "u64_hex")]
    pub amount: u64,
}

impl Withdrawal {
    /// Return the withdrawal amount in wei.
    pub fn amount_wei(&self) -> U256 {
        U256::from(self.amount) * U256::from(GWEI_TO_WEI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // <https://github.com/paradigmxyz/reth/issues/1614>
    #[test]
    fn test_withdrawal_serde_roundtrip() {
        let input = r#"[{"index":"0x0","validatorIndex":"0x0","address":"0x0000000000000000000000000000000000001000","amount":"0x1"},{"index":"0x1","validatorIndex":"0x1","address":"0x0000000000000000000000000000000000001001","amount":"0x1"},{"index":"0x2","validatorIndex":"0x2","address":"0x0000000000000000000000000000000000001002","amount":"0x1"},{"index":"0x3","validatorIndex":"0x3","address":"0x0000000000000000000000000000000000001003","amount":"0x1"},{"index":"0x4","validatorIndex":"0x4","address":"0x0000000000000000000000000000000000001004","amount":"0x1"},{"index":"0x5","validatorIndex":"0x5","address":"0x0000000000000000000000000000000000001005","amount":"0x1"},{"index":"0x6","validatorIndex":"0x6","address":"0x0000000000000000000000000000000000001006","amount":"0x1"},{"index":"0x7","validatorIndex":"0x7","address":"0x0000000000000000000000000000000000001007","amount":"0x1"},{"index":"0x8","validatorIndex":"0x8","address":"0x0000000000000000000000000000000000001008","amount":"0x1"},{"index":"0x9","validatorIndex":"0x9","address":"0x0000000000000000000000000000000000001009","amount":"0x1"},{"index":"0xa","validatorIndex":"0xa","address":"0x000000000000000000000000000000000000100a","amount":"0x1"},{"index":"0xb","validatorIndex":"0xb","address":"0x000000000000000000000000000000000000100b","amount":"0x1"},{"index":"0xc","validatorIndex":"0xc","address":"0x000000000000000000000000000000000000100c","amount":"0x1"},{"index":"0xd","validatorIndex":"0xd","address":"0x000000000000000000000000000000000000100d","amount":"0x1"},{"index":"0xe","validatorIndex":"0xe","address":"0x000000000000000000000000000000000000100e","amount":"0x1"},{"index":"0xf","validatorIndex":"0xf","address":"0x000000000000000000000000000000000000100f","amount":"0x1"}]"#;

        let withdrawals: Vec<Withdrawal> = serde_json::from_str(input).unwrap();
        let s = serde_json::to_string(&withdrawals).unwrap();
        assert_eq!(input, s);
    }
}
