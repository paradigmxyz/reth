use crate::{Bytes, B256, U256};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, fmt::Write};

/// A storage key type that can be serialized to and from a hex string up to 32 bytes. Used for
/// `eth_getStorageAt` and `eth_getProof` RPCs.
///
/// This is a wrapper type meant to mirror geth's serialization and deserialization behavior for
/// storage keys.
///
/// In `eth_getStorageAt`, this is used for deserialization of the `index` field. Internally, the
/// index is a [B256], but in `eth_getStorageAt` requests, its serialization can be _up to_ 32
/// bytes. To support this, the storage key is deserialized first as a U256, and converted to a
/// B256 for use internally.
///
/// `eth_getProof` also takes storage keys up to 32 bytes as input, so the `keys` field is
/// similarly deserialized. However, geth populates the storage proof `key` fields in the response
/// by mirroring the `key` field used in the input.
///  * See how `storageKey`s (the input) are populated in the `StorageResult` (the output):
///  <https://github.com/ethereum/go-ethereum/blob/00a73fbcce3250b87fc4160f3deddc44390848f4/internal/ethapi/api.go#L658-L690>
///
/// The contained [B256] and From implementation for String are used to preserve the input and
/// implement this behavior from geth.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "U256", into = "String")]
pub struct JsonStorageKey(pub B256);

impl From<U256> for JsonStorageKey {
    fn from(value: U256) -> Self {
        // SAFETY: Address (B256) and U256 have the same number of bytes
        JsonStorageKey(B256::from(value.to_be_bytes()))
    }
}

impl From<JsonStorageKey> for String {
    fn from(value: JsonStorageKey) -> Self {
        // SAFETY: Address (B256) and U256 have the same number of bytes
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

/// Converts a Bytes value into a B256, accepting inputs that are less than 32 bytes long. These
/// inputs will be left padded with zeros.
pub fn from_bytes_to_b256<'de, D>(bytes: Bytes) -> Result<B256, D::Error>
where
    D: Deserializer<'de>,
{
    if bytes.0.len() > 32 {
        return Err(serde::de::Error::custom("input too long to be a B256"))
    }

    // left pad with zeros to 32 bytes
    let mut padded = [0u8; 32];
    padded[32 - bytes.0.len()..].copy_from_slice(&bytes.0);

    // then convert to B256 without a panic
    Ok(B256::from_slice(&padded))
}

/// Deserializes the input into an Option<HashMap<B256, B256>>, using [from_bytes_to_b256] which
/// allows cropped values:
///
/// ```json
///  {
///      "0x0000000000000000000000000000000000000000000000000000000000000001": "0x22"
///   }
/// ```
pub fn deserialize_storage_map<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<B256, B256>>, D::Error>
where
    D: Deserializer<'de>,
{
    let map = Option::<HashMap<Bytes, Bytes>>::deserialize(deserializer)?;
    match map {
        Some(mut map) => {
            let mut res_map = HashMap::with_capacity(map.len());
            for (k, v) in map.drain() {
                let k_deserialized = from_bytes_to_b256::<'de, D>(k)?;
                let v_deserialized = from_bytes_to_b256::<'de, D>(v)?;
                res_map.insert(k_deserialized, v_deserialized);
            }
            Ok(Some(res_map))
        }
        None => Ok(None),
    }
}
