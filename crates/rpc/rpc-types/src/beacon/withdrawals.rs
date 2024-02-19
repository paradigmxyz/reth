use crate::Withdrawal;
use alloy_primitives::Address;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, SerializeAs};

/// Same as [Withdrawal] but respects the Beacon API format which uses snake-case and quoted
/// decimals.
#[serde_as]
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct BeaconWithdrawal {
    #[serde_as(as = "DisplayFromStr")]
    index: u64,
    #[serde_as(as = "DisplayFromStr")]
    validator_index: u64,
    address: Address,
    #[serde_as(as = "DisplayFromStr")]
    amount: u64,
}

impl SerializeAs<Withdrawal> for BeaconWithdrawal {
    fn serialize_as<S>(source: &Withdrawal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        beacon_withdrawals::serialize(source, serializer)
    }
}

impl<'de> DeserializeAs<'de, Withdrawal> for BeaconWithdrawal {
    fn deserialize_as<D>(deserializer: D) -> Result<Withdrawal, D::Error>
    where
        D: Deserializer<'de>,
    {
        beacon_withdrawals::deserialize(deserializer)
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_withdrawals {
    use super::*;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(payload_attributes: &Withdrawal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let withdrawal = BeaconWithdrawal {
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
        let withdrawal = BeaconWithdrawal::deserialize(deserializer)?;
        Ok(Withdrawal {
            index: withdrawal.index,
            validator_index: withdrawal.validator_index,
            address: withdrawal.address,
            amount: withdrawal.amount,
        })
    }
}
