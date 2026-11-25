//! Validation of RPC `blockOverrides` objects.
//!
//! alloy's [`BlockOverrides`] drops keys it does not know when it is decoded, so callers that must
//! reject unknown keys check the raw object's keys with [`deserialize_block_override_keys`].

use crate::EthApiError;
use alloy_rpc_types_eth::BlockOverrides;
use reth_chainspec::EthereumHardforks;
use serde::{
    de::{Error, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserializer,
};
use std::fmt;

/// The `blockOverrides` keys that decode into alloy's [`BlockOverrides`], including its aliases.
///
/// Keys are matched exactly. A key that differs from one of these only in case is unsupported, so
/// two keys that differ only in case are always rejected.
const SUPPORTED_BLOCK_OVERRIDE_KEYS: &[&str] = &[
    "number",
    "blockNumber",
    "difficulty",
    "time",
    "timestamp",
    "gasLimit",
    "coinbase",
    "feeRecipient",
    "random",
    "prevRandao",
    "baseFee",
    "baseFeePerGas",
    "blobBaseFee",
    "beaconRoot",
    "parentBeaconBlockRoot",
    "withdrawals",
    "blockHash",
];

/// Returns [`EthApiError::InvalidParams`] for the first key of a `blockOverrides` object that is
/// not in [`SUPPORTED_BLOCK_OVERRIDE_KEYS`].
///
/// The execution-apis `BlockOverrides` schema requires a client to reject a key it does not
/// support with -32602: <https://github.com/ethereum/execution-apis/pull/944>
fn ensure_supported_block_override_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), EthApiError> {
    match keys.into_iter().find(|key| !SUPPORTED_BLOCK_OVERRIDE_KEYS.contains(key)) {
        // The key comes from the request, so only its first 64 characters are echoed back.
        Some(key) => {
            Err(EthApiError::InvalidParams(format!("unsupported block override key: {key:.64}")))
        }
        None => Ok(()),
    }
}

/// Deserializes a `blockOverrides` value without keeping it, failing with -32602 on the first key
/// that does not decode into alloy's [`BlockOverrides`], aliases included. Keys are matched
/// exactly, so a key that differs from a supported one only in case is rejected, and so are two
/// keys that differ only in case. The execution-apis `BlockOverrides` schema requires both:
/// <https://github.com/ethereum/execution-apis/pull/944>
///
/// Use it with `#[serde(deserialize_with = "...")]` on a `()` field in a second pass over raw
/// input that has already decoded into alloy's types, so the value itself is known to be valid.
/// `null` and the positional array form, which [`BlockOverrides`] also accepts, have no keys and
/// pass.
pub fn deserialize_block_override_keys<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<(), D::Error> {
    deserializer.deserialize_any(BlockOverrideKeysVisitor)
}

/// Returns [`EthApiError::InvalidParams`] if `block_overrides` sets `difficulty` and the merge is
/// active at `block_number`.
///
/// After the merge the difficulty of a block is 0, and the execution-apis `BlockOverrides` schema
/// requires rejecting the key with -32602 for such a block, whatever its value:
/// <https://github.com/ethereum/execution-apis/pull/944>. alloy decodes `"difficulty": null` as a
/// missing key, so it passes.
///
/// `block_number` must be the block whose fork rules the request executes under: the traced block
/// for a call, whatever `number` it overrides, or the number of each simulated block.
pub fn ensure_no_post_merge_difficulty_override(
    chain_spec: &impl EthereumHardforks,
    block_number: u64,
    block_overrides: &BlockOverrides,
) -> Result<(), EthApiError> {
    if block_overrides.difficulty.is_some() && chain_spec.is_paris_active_at_block(block_number) {
        return Err(EthApiError::InvalidParams(String::from(
            "difficulty block override is not supported after the merge",
        )))
    }
    Ok(())
}

/// Checks the keys of a `blockOverrides` value, see [`deserialize_block_override_keys`].
struct BlockOverrideKeysVisitor;

impl<'de> Visitor<'de> for BlockOverrideKeysVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("block overrides")
    }

    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        // The input already decoded into alloy's types, which reject repeated supported keys, so
        // this allocates a bounded number of keys before it returns or hits an unsupported one.
        while let Some(key) = map.next_key::<String>()? {
            ensure_supported_block_override_keys([key.as_str()]).map_err(A::Error::custom)?;
            map.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // The positional form has no keys.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};

    #[test]
    fn supported_keys_match_alloy() {
        // Fails to compile when alloy adds a field, so the key list can't miss one.
        let BlockOverrides {
            number: _,
            difficulty: _,
            time: _,
            gas_limit: _,
            coinbase: _,
            random: _,
            base_fee: _,
            blob_base_fee: _,
            beacon_root: _,
            withdrawals: _,
            block_hash: _,
        } = BlockOverrides::default();

        for key in SUPPORTED_BLOCK_OVERRIDE_KEYS {
            let value = match *key {
                "withdrawals" => serde_json::json!([]),
                "blockHash" => serde_json::json!({ "1": format!("0x{}", "11".repeat(32)) }),
                "coinbase" | "feeRecipient" => serde_json::json!(format!("0x{}", "11".repeat(20))),
                "random" | "prevRandao" | "beaconRoot" | "parentBeaconBlockRoot" => {
                    serde_json::json!(format!("0x{}", "11".repeat(32)))
                }
                _ => serde_json::json!("0x1"),
            };
            let object = serde_json::Map::from_iter([(key.to_string(), value)]);
            let overrides: BlockOverrides = serde_json::from_value(object.into()).unwrap();
            assert_ne!(overrides, BlockOverrides::default(), "alloy ignores key {key}");
        }
    }

    #[test]
    fn supported_keys_are_alloy_fields() {
        // serde passes a struct's field names and aliases to `deserialize_struct`, so capture them
        // to catch a field or alias alloy adds.
        struct Fields(Vec<&'static str>);

        impl<'de> Deserializer<'de> for &mut Fields {
            type Error = serde::de::value::Error;

            fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
                Err(Error::custom("not a struct"))
            }

            fn deserialize_struct<V: Visitor<'de>>(
                self,
                _: &'static str,
                fields: &'static [&'static str],
                _: V,
            ) -> Result<V::Value, Self::Error> {
                self.0.extend(fields);
                Err(Error::custom("captured"))
            }

            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
                byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
                identifier ignored_any
            }
        }

        let mut fields = Fields(Vec::new());
        let _ = <BlockOverrides as serde::Deserialize>::deserialize(&mut fields);
        let mut alloy = fields.0;
        let mut supported = SUPPORTED_BLOCK_OVERRIDE_KEYS.to_vec();
        alloy.sort_unstable();
        supported.sort_unstable();
        assert_eq!(supported, alloy);
    }

    #[test]
    fn rejects_unknown_and_case_variant_keys() {
        assert!(ensure_supported_block_override_keys(["time", "blockHash"]).is_ok());
        for (keys, key) in [
            (&["fooBar"][..], "fooBar"),
            (&["Time"], "Time"),
            (&["time", "Time"], "Time"),
            (&["blockhash"], "blockhash"),
        ] {
            let err = ensure_supported_block_override_keys(keys.iter().copied()).unwrap_err();
            let EthApiError::InvalidParams(msg) = err else { panic!("unexpected error: {err:?}") };
            assert_eq!(msg, format!("unsupported block override key: {key}"));
        }
    }

    #[test]
    fn truncates_echoed_key() {
        let key = "k".repeat(1000);
        let err = ensure_supported_block_override_keys([key.as_str()]).unwrap_err();
        let EthApiError::InvalidParams(msg) = err else { panic!("unexpected error: {err:?}") };
        assert_eq!(msg, format!("unsupported block override key: {}", "k".repeat(64)));
    }

    #[test]
    fn rejects_difficulty_only_after_the_merge() {
        let difficulty = BlockOverrides { difficulty: Some(U256::ZERO), ..Default::default() };
        let time = BlockOverrides { time: Some(1), ..Default::default() };
        let paris = MAINNET.paris_block_and_final_difficulty.unwrap().0;

        assert!(ensure_no_post_merge_difficulty_override(&*MAINNET, paris - 1, &difficulty).is_ok());
        let err =
            ensure_no_post_merge_difficulty_override(&*MAINNET, paris, &difficulty).unwrap_err();
        let EthApiError::InvalidParams(msg) = err else { panic!("unexpected error: {err:?}") };
        assert_eq!(msg, "difficulty block override is not supported after the merge");
        assert!(ensure_no_post_merge_difficulty_override(&*MAINNET, paris, &time).is_ok());

        let no_merge = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .london_activated()
            .build();
        assert!(ensure_no_post_merge_difficulty_override(&no_merge, u64::MAX, &difficulty).is_ok());
    }
}
