use reth_primitives::{BlockNumber, BlockNumberOrTag, TxIndex};
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};

/// An inclusive range of blocks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BlockRange {
    first_block: BlockNumberOrTag,
    last_block: BlockNumberOrTag,
}

/// An address_getAppearances method response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddressAppearances(Vec<RelevantTransaction>);

/// A transaction identifier used in the address_getAppearances method response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelevantTransaction {
    #[serde(serialize_with = "ser_hex_u64", deserialize_with = "de_hex_u64")]
    block_number: BlockNumber,
    #[serde(serialize_with = "ser_hex_u64", deserialize_with = "de_hex_u64")]
    transaction_index: TxIndex,
}

fn ser_hex_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("0x{:x}", value))
}

fn de_hex_u64<'de, D>(deserializer: D) -> Result<BlockNumber, D::Error>
where
    D: Deserializer<'de>,
{
    struct Hexu64Visitor;
    impl<'de> Visitor<'de> for Hexu64Visitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a hexadecimal u64 string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u64::from_str_radix(value.trim_start_matches("0x"), 16)
                .map_err(|_err| serde::de::Error::custom("invalid hexadecimal u64 string"))
        }
    }
    deserializer.deserialize_str(Hexu64Visitor)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn serde_block_range() {
        let range = BlockRange {
            first_block: BlockNumberOrTag::Number(17190873),
            last_block: BlockNumberOrTag::Number(17190889),
        };

        let serialized = serde_json::to_string(&range).unwrap();
        assert_eq!(serialized, r#"{"firstBlock":"0x1064fd9","lastBlock":"0x1064fe9"}"#);
        let deserialized: BlockRange = serde_json::from_str(&serialized).unwrap();
        assert_eq!(range, deserialized);
    }

    #[test]
    fn serde_block_range_tags() {
        let range = BlockRange {
            first_block: BlockNumberOrTag::Finalized,
            last_block: BlockNumberOrTag::Latest,
        };

        let serialized = serde_json::to_string(&range).unwrap();
        assert_eq!(serialized, r#"{"firstBlock":"finalized","lastBlock":"latest"}"#);
        let deserialized: BlockRange = serde_json::from_str(&serialized).unwrap();
        assert_eq!(range, deserialized);
    }

    #[test]
    fn serde_appearances() {
        let appearances = AddressAppearances(vec![
            RelevantTransaction { block_number: 17190873, transaction_index: 95 },
            RelevantTransaction { block_number: 17190889, transaction_index: 89 },
        ]);

        let serialized = serde_json::to_string(&appearances).unwrap();
        assert_eq!(
            serialized,
            r#"[{"blockNumber":"0x1064fd9","transactionIndex":"0x5f"},{"blockNumber":"0x1064fe9","transactionIndex":"0x59"}]"#
        );
        let deserialized: AddressAppearances = serde_json::from_str(&serialized).unwrap();
        assert_eq!(appearances, deserialized);
    }
}
