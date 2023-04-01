use crate::{H256, U256};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// A storage key type that can be serialized to and from a hex string up to 40 characters. Used
/// for `eth_getStorageAt` and `eth_getProof` RPCs.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "U256", into = "String")]
pub struct JsonStorageKey(pub H256);

impl From<U256> for JsonStorageKey {
    fn from(value: U256) -> Self {
        // SAFETY: Address (H256) and U256 have the same number of bytes
        JsonStorageKey(H256::from(value.to_be_bytes()))
    }
}

impl From<JsonStorageKey> for String {
    fn from(value: JsonStorageKey) -> Self {
        // SAFETY: Address (H256) and U256 have the same number of bytes
        let uint = U256::from_be_bytes(value.0 .0);

        // serialize byte by byte
        //
        // this is mainly so we can return an output that hive testing expects, because the
        // `eth_getProof` implementation in geth simply mirrors the input
        //
        // see the use of `hexKey` in the `eth_getProof` response:
        // <https://github.com/ethereum/go-ethereum/blob/00a73fbcce3250b87fc4160f3deddc44390848f4/internal/ethapi/api.go#L658-L690>
        let bytes = uint.to_be_bytes_trimmed_vec();
        let mut hex = String::with_capacity(2 + bytes.len() * 2);
        hex.push_str("0x");
        for byte in bytes {
            write!(hex, "{:02x}", byte).unwrap();
        }
        hex
    }
}
