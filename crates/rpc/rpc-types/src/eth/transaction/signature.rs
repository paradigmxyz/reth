//! Signature related RPC values
use reth_primitives::U256;
use serde::{Deserialize, Serialize};

/// Container type for all signature fields in RPC
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    // TODO: change these fields to an untagged enum for `v` XOR `y_parity` if/when CLs support it.
    // See <https://github.com/ethereum/go-ethereum/issues/27727> for more information
    /// For EIP-155, EIP-2930 and Blob transactions this is set to the parity (0 for even, 1 for
    /// odd) of the y-value of the secp256k1 signature.
    ///
    /// For legacy transactions, this is the recovery id
    ///
    /// See also <https://ethereum.github.io/execution-apis/api-documentation/> and <https://ethereum.org/en/developers/docs/apis/json-rpc/#eth_gettransactionbyhash>
    pub v: U256,
    /// The y parity of the signature. This is only used for typed (non-legacy) transactions.
    #[serde(default, rename = "yParity", skip_serializing_if = "Option::is_none")]
    pub y_parity: Option<Parity>,
}

/// Type that represents the signature parity byte, meant for use in RPC.
///
/// This will be serialized as "0x0" if false, and "0x1" if true.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Parity(
    #[serde(serialize_with = "serialize_parity", deserialize_with = "deserialize_parity")] pub bool,
);

fn serialize_parity<S>(parity: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(if *parity { "0x1" } else { "0x0" })
}

/// This implementation disallows serialization of the y parity bit that are not `"0x0"` or `"0x1"`.
fn deserialize_parity<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "0x0" => Ok(false),
        "0x1" => Ok(true),
        _ => Err(serde::de::Error::custom(format!(
            "invalid parity value, parity should be either \"0x0\" or \"0x1\": {}",
            s
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn deserialize_without_parity() {
        let raw_signature_without_y_parity = r#"{
            "r":"0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0",
            "s":"0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05",
            "v":"0x1"
        }"#;

        let signature: Signature = serde_json::from_str(raw_signature_without_y_parity).unwrap();
        let expected = Signature {
            r: U256::from_str("0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0")
                .unwrap(),
            s: U256::from_str("0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05")
                .unwrap(),
            v: U256::from_str("1").unwrap(),
            y_parity: None,
        };

        assert_eq!(signature, expected);
    }

    #[test]
    fn deserialize_with_parity() {
        let raw_signature_with_y_parity = r#"{
            "r":"0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0",
            "s":"0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05",
            "v":"0x1",
            "yParity": "0x1"
        }"#;

        let signature: Signature = serde_json::from_str(raw_signature_with_y_parity).unwrap();
        let expected = Signature {
            r: U256::from_str("0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0")
                .unwrap(),
            s: U256::from_str("0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05")
                .unwrap(),
            v: U256::from_str("1").unwrap(),
            y_parity: Some(Parity(true)),
        };

        assert_eq!(signature, expected);
    }

    #[test]
    fn serialize_both_parity() {
        // this test should be removed if the struct moves to an enum based on tx type
        let signature = Signature {
            r: U256::from_str("0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0")
                .unwrap(),
            s: U256::from_str("0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05")
                .unwrap(),
            v: U256::from_str("1").unwrap(),
            y_parity: Some(Parity(true)),
        };

        let serialized = serde_json::to_string(&signature).unwrap();
        assert_eq!(
            serialized,
            r#"{"r":"0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0","s":"0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05","v":"0x1","yParity":"0x1"}"#
        );
    }

    #[test]
    fn serialize_v_only() {
        // this test should be removed if the struct moves to an enum based on tx type
        let signature = Signature {
            r: U256::from_str("0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0")
                .unwrap(),
            s: U256::from_str("0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05")
                .unwrap(),
            v: U256::from_str("1").unwrap(),
            y_parity: None,
        };

        let expected = r#"{"r":"0xc569c92f176a3be1a6352dd5005bfc751dcb32f57623dd2a23693e64bf4447b0","s":"0x1a891b566d369e79b7a66eecab1e008831e22daa15f91a0a0cf4f9f28f47ee05","v":"0x1"}"#;

        let serialized = serde_json::to_string(&signature).unwrap();
        assert_eq!(serialized, expected);
    }

    #[test]
    fn serialize_parity() {
        let parity = Parity(true);
        let serialized = serde_json::to_string(&parity).unwrap();
        assert_eq!(serialized, r#""0x1""#);

        let parity = Parity(false);
        let serialized = serde_json::to_string(&parity).unwrap();
        assert_eq!(serialized, r#""0x0""#);
    }

    #[test]
    fn deserialize_parity() {
        let raw_parity = r#""0x1""#;
        let parity: Parity = serde_json::from_str(raw_parity).unwrap();
        assert_eq!(parity, Parity(true));

        let raw_parity = r#""0x0""#;
        let parity: Parity = serde_json::from_str(raw_parity).unwrap();
        assert_eq!(parity, Parity(false));
    }

    #[test]
    fn deserialize_parity_invalid() {
        let raw_parity = r#""0x2""#;
        let parity: Result<Parity, _> = serde_json::from_str(raw_parity);
        assert!(parity.is_err());

        let raw_parity = r#""0x""#;
        let parity: Result<Parity, _> = serde_json::from_str(raw_parity);
        assert!(parity.is_err());

        // In the spec this is defined as a uint, which requires 0x
        // yParity:
        // <https://github.com/ethereum/execution-apis/blob/8fcafbbc86257f6e61fddd9734148e38872a71c9/src/schemas/transaction.yaml#L157>
        //
        // uint:
        // <https://github.com/ethereum/execution-apis/blob/8fcafbbc86257f6e61fddd9734148e38872a71c9/src/schemas/base-types.yaml#L47>
        let raw_parity = r#""1""#;
        let parity: Result<Parity, _> = serde_json::from_str(raw_parity);
        assert!(parity.is_err());

        let raw_parity = r#""0""#;
        let parity: Result<Parity, _> = serde_json::from_str(raw_parity);
        assert!(parity.is_err());
    }
}
