//! Withdrawal type and serde helpers.

use std::mem;

use crate::serde_helpers::u64_hex;
use alloy_primitives::{Address, U256};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, SerializeAs};

/// Multiplier for converting gwei to wei.
pub const GWEI_TO_WEI: u64 = 1_000_000_000;

/// Withdrawal represents a validator withdrawal from the consensus layer.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Hash, RlpEncodable, RlpDecodable, Serialize, Deserialize,
)]
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

    /// Calculate a heuristic for the in-memory size of the [Withdrawal].
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<Self>()
    }
}

/// Same as [Withdrawal] but respects the Beacon API format which uses snake-case and quoted
/// decimals.
#[serde_as]
#[derive(Serialize, Deserialize)]
pub(crate) struct BeaconAPIWithdrawal {
    #[serde_as(as = "DisplayFromStr")]
    index: u64,
    #[serde_as(as = "DisplayFromStr")]
    validator_index: u64,
    address: Address,
    #[serde_as(as = "DisplayFromStr")]
    amount: u64,
}

impl SerializeAs<Withdrawal> for BeaconAPIWithdrawal {
    fn serialize_as<S>(source: &Withdrawal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        beacon_api_withdrawals::serialize(source, serializer)
    }
}

impl<'de> DeserializeAs<'de, Withdrawal> for BeaconAPIWithdrawal {
    fn deserialize_as<D>(deserializer: D) -> Result<Withdrawal, D::Error>
    where
        D: Deserializer<'de>,
    {
        beacon_api_withdrawals::deserialize(deserializer)
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_api_withdrawals {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(payload_attributes: &Withdrawal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let withdrawal = BeaconAPIWithdrawal {
            index: payload_attributes.index,
            validator_index: payload_attributes.validator_index,
            address: payload_attributes.address,
            amount: payload_attributes.amount,
        };
        withdrawal.serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Withdrawal, D::Error>
    where
        D: Deserializer<'de>,
    {
        let withdrawal = BeaconAPIWithdrawal::deserialize(deserializer)?;
        Ok(Withdrawal {
            index: withdrawal.index,
            validator_index: withdrawal.validator_index,
            address: withdrawal.address,
            amount: withdrawal.amount,
        })
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
