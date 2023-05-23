use crate::{H256, U256};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// A storage key type that can be serialized to and from a hex string up to 32 bytes. Used for
/// `eth_getStorageAt` and `eth_getProof` RPCs.
///
/// This is a wrapper type meant to mirror geth's serialization and deserialization behavior for
/// storage keys.
///
/// In `eth_getStorageAt`, this is used for deserialization of the `index` field. Internally, the
/// index is a [H256], but in `eth_getStorageAt` requests, its serialization can be _up to_ 32
/// bytes. To support this, the storage key is deserialized first as a U256, and converted to a
/// H256 for use internally.
///
/// `eth_getProof` also takes storage keys up to 32 bytes as input, so the `keys` field is
/// similarly deserialized. However, geth populates the storage proof `key` fields in the response
/// by mirroring the `key` field used in the input.
///  * See how `storageKey`s (the input) are populated in the `StorageResult` (the output):
///  <https://github.com/ethereum/go-ethereum/blob/00a73fbcce3250b87fc4160f3deddc44390848f4/internal/ethapi/api.go#L658-L690>
///
/// The contained [H256] and From implementation for String are used to preserve the input and
/// implement this behavior from geth.
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
