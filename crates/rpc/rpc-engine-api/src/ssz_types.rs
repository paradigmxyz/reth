//! SSZ types for the REST `POST /new-payload-with-witness` endpoint.
//!
//! Defines the SSZ encoding for [`NewPayloadWithWitnessResponseV1`] as specified in
//! <https://github.com/developeruche/execution-apis/blob/feat/new-payload-with-witness/src/engine/rest.md>.
//!
//! We implement manual SSZ serialization because these types use SSZ `Union` variants
//! which are not natively supported by `ethereum_ssz` derive macros.

use alloy_primitives::B256;
use alloy_rpc_types_debug::ExecutionWitness;
use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};

/// Maximum number of `uint8` elements in the `validation_error` union list variant.
const VALIDATION_ERROR_MAX: usize = 8192;

/// Maps a [`PayloadStatusEnum`] to the SSZ `uint8` status code.
fn status_to_u8(status: &PayloadStatusEnum) -> u8 {
    match status {
        PayloadStatusEnum::Valid => 0,
        PayloadStatusEnum::Invalid { .. } => 1,
        PayloadStatusEnum::Syncing => 2,
        PayloadStatusEnum::Accepted => 3,
    }
}

/// SSZ-encode a `Union[None, ByteVector[32]]` for `latest_valid_hash`.
///
/// SSZ Union encoding: selector byte followed by the variant value.
/// `None` variant → selector `0x00`, no further bytes.
/// `ByteVector[32]` variant → selector `0x01`, then 32 raw bytes.
fn encode_optional_hash(hash: Option<B256>) -> Vec<u8> {
    match hash {
        None => vec![0u8],
        Some(h) => {
            let mut buf = Vec::with_capacity(33);
            buf.push(1u8);
            buf.extend_from_slice(h.as_slice());
            buf
        }
    }
}

/// SSZ-encode a `Union[None, List[uint8, VALIDATION_ERROR_MAX]]` for `validation_error`.
///
/// `None` variant → selector `0x00`.
/// `List[uint8, …]` variant → selector `0x01`, then the UTF-8 bytes (truncated if needed).
fn encode_optional_validation_error(error: Option<&str>) -> Vec<u8> {
    match error {
        None => vec![0u8],
        Some(s) => {
            let bytes = s.as_bytes();
            let truncated = truncate_utf8(bytes, VALIDATION_ERROR_MAX);
            let mut buf = Vec::with_capacity(1 + truncated.len());
            buf.push(1u8);
            buf.extend_from_slice(truncated);
            buf
        }
    }
}

/// Truncates a UTF-8 byte slice to at most `max_len` bytes without splitting
/// a multi-byte code point.
fn truncate_utf8(bytes: &[u8], max_len: usize) -> &[u8] {
    if bytes.len() <= max_len {
        return bytes;
    }
    // Walk backwards from max_len to find a valid UTF-8 boundary.
    let mut end = max_len;
    while end > 0 && !is_utf8_char_boundary(bytes[end]) {
        end -= 1;
    }
    &bytes[..end]
}

/// Returns `true` if the byte is a UTF-8 character boundary (not a continuation byte).
const fn is_utf8_char_boundary(b: u8) -> bool {
    // Continuation bytes have the pattern 10xxxxxx (0x80..0xBF).
    (b as i8) >= -0x40
}

/// SSZ-encode a `List[List[uint8, MAX], MAX]` — a list of variable-length byte lists.
///
/// SSZ encoding for a variable-length list of variable-length elements uses an offset
/// table followed by concatenated element data.
fn encode_list_of_byte_lists(items: &[alloy_primitives::Bytes]) -> Vec<u8> {
    if items.is_empty() {
        return Vec::new();
    }

    // Each element in the offset table is a 4-byte little-endian offset.
    let offset_table_size = items.len() * 4;
    let total_data_size: usize = items.iter().map(|item| item.len()).sum();
    let mut buf = Vec::with_capacity(offset_table_size + total_data_size);

    // Write the offset table.
    let mut current_offset = offset_table_size;
    for item in items {
        buf.extend_from_slice(&(current_offset as u32).to_le_bytes());
        current_offset += item.len();
    }

    // Write the data.
    for item in items {
        buf.extend_from_slice(item);
    }

    buf
}

/// SSZ-encode a `Union[None, ExecutionWitnessV1]` for the `witness` field.
///
/// `None` variant → selector `0x00`.
/// `ExecutionWitnessV1` variant → selector `0x01`, then the SSZ Container encoding of the witness.
fn encode_optional_witness(witness: Option<&ExecutionWitness>) -> Vec<u8> {
    match witness {
        None => vec![0u8],
        Some(w) => {
            let state_encoded = encode_list_of_byte_lists(&w.state);
            let codes_encoded = encode_list_of_byte_lists(&w.codes);
            let headers_encoded = encode_list_of_byte_lists(&w.headers);

            // SSZ Container with 3 variable-length fields:
            // offset_table (3 × 4 bytes) + concatenated field data
            let offset_table_size = 3 * 4;
            let total_size =
                1 + offset_table_size + state_encoded.len() + codes_encoded.len() + headers_encoded.len();
            let mut buf = Vec::with_capacity(total_size);

            // Union selector
            buf.push(1u8);

            // Container offset table
            let state_offset = offset_table_size as u32;
            let codes_offset = state_offset + state_encoded.len() as u32;
            let headers_offset = codes_offset + codes_encoded.len() as u32;

            buf.extend_from_slice(&state_offset.to_le_bytes());
            buf.extend_from_slice(&codes_offset.to_le_bytes());
            buf.extend_from_slice(&headers_offset.to_le_bytes());

            // Container data
            buf.extend_from_slice(&state_encoded);
            buf.extend_from_slice(&codes_encoded);
            buf.extend_from_slice(&headers_encoded);

            buf
        }
    }
}

/// Encodes a [`PayloadStatus`] and an optional [`ExecutionWitness`] into the SSZ
/// serialization of `NewPayloadWithWitnessResponseV1`.
///
/// The SSZ Container has 4 fields:
/// - `status`: `uint8` (fixed, 1 byte)
/// - `latest_valid_hash`: `Union[None, ByteVector[32]]` (variable)
/// - `validation_error`: `Union[None, List[uint8, 8192]]` (variable)
/// - `witness`: `Union[None, ExecutionWitnessV1]` (variable)
///
/// Fixed-size fields come first, followed by offset pointers for variable-size fields,
/// then the variable-size data.
pub fn encode_new_payload_with_witness_response(
    status: &PayloadStatus,
    witness: Option<&ExecutionWitness>,
) -> Vec<u8> {
    // Extract validation_error string from the status enum if present.
    let validation_error = status.status.validation_error();

    // Encode variable-length fields
    let hash_encoded = encode_optional_hash(status.latest_valid_hash);
    let error_encoded = encode_optional_validation_error(validation_error);
    let witness_encoded = encode_optional_witness(witness);

    // SSZ Container layout:
    //   Fixed part:
    //     status: 1 byte
    //     latest_valid_hash offset: 4 bytes
    //     validation_error offset: 4 bytes
    //     witness offset: 4 bytes
    //   Variable part:
    //     latest_valid_hash data
    //     validation_error data
    //     witness data
    let fixed_size = 1 + 4 + 4 + 4; // 13 bytes

    let hash_offset = fixed_size as u32;
    let error_offset = hash_offset + hash_encoded.len() as u32;
    let witness_offset = error_offset + error_encoded.len() as u32;

    let total_size = fixed_size + hash_encoded.len() + error_encoded.len() + witness_encoded.len();
    let mut buf = Vec::with_capacity(total_size);

    // Fixed part
    buf.push(status_to_u8(&status.status));
    buf.extend_from_slice(&hash_offset.to_le_bytes());
    buf.extend_from_slice(&error_offset.to_le_bytes());
    buf.extend_from_slice(&witness_offset.to_le_bytes());

    // Variable part
    buf.extend_from_slice(&hash_encoded);
    buf.extend_from_slice(&error_encoded);
    buf.extend_from_slice(&witness_encoded);

    buf
}

/// Extension trait for extracting validation error strings from [`PayloadStatusEnum`].
#[allow(dead_code)]
pub(crate) trait ValidationErrorExt {
    /// Returns the validation error string, if the status is an error variant.
    fn validation_error(&self) -> Option<&str>;
}

impl ValidationErrorExt for PayloadStatusEnum {
    fn validation_error(&self) -> Option<&str> {
        match self {
            Self::Invalid { validation_error } => Some(validation_error.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};

    #[test]
    fn test_status_to_u8() {
        assert_eq!(status_to_u8(&PayloadStatusEnum::Valid), 0);
        assert_eq!(
            status_to_u8(&PayloadStatusEnum::Invalid { validation_error: String::new() }),
            1
        );
        assert_eq!(status_to_u8(&PayloadStatusEnum::Syncing), 2);
        assert_eq!(status_to_u8(&PayloadStatusEnum::Accepted), 3);
    }

    #[test]
    fn test_encode_optional_hash_none() {
        let encoded = encode_optional_hash(None);
        assert_eq!(encoded, vec![0u8]);
    }

    #[test]
    fn test_encode_optional_hash_some() {
        let hash = B256::repeat_byte(0xAB);
        let encoded = encode_optional_hash(Some(hash));
        assert_eq!(encoded.len(), 33);
        assert_eq!(encoded[0], 1u8);
        assert_eq!(&encoded[1..], hash.as_slice());
    }

    #[test]
    fn test_encode_optional_validation_error_none() {
        let encoded = encode_optional_validation_error(None);
        assert_eq!(encoded, vec![0u8]);
    }

    #[test]
    fn test_encode_optional_validation_error_some() {
        let encoded = encode_optional_validation_error(Some("block is invalid"));
        assert_eq!(encoded[0], 1u8);
        assert_eq!(&encoded[1..], b"block is invalid");
    }

    #[test]
    fn test_truncate_utf8_within_limit() {
        let s = "hello";
        assert_eq!(truncate_utf8(s.as_bytes(), 10), b"hello");
    }

    #[test]
    fn test_truncate_utf8_exact() {
        let s = "hello";
        assert_eq!(truncate_utf8(s.as_bytes(), 5), b"hello");
    }

    #[test]
    fn test_truncate_utf8_multibyte() {
        // '€' is 3 bytes in UTF-8: E2 82 AC
        let s = "€€";
        // 6 bytes total, truncate to 4 → should keep only first '€' (3 bytes)
        let result = truncate_utf8(s.as_bytes(), 4);
        assert_eq!(result.len(), 3);
        assert_eq!(result, "€".as_bytes());
    }

    #[test]
    fn test_encode_valid_response_without_witness() {
        let status = PayloadStatus::new(
            PayloadStatusEnum::Valid,
            Some(B256::ZERO),
        );
        let encoded = encode_new_payload_with_witness_response(&status, None);
        // Should not panic, and should be non-empty
        assert!(!encoded.is_empty());
        // First byte is status = 0 (VALID)
        assert_eq!(encoded[0], 0u8);
    }

    #[test]
    fn test_encode_syncing_response() {
        let status = PayloadStatus::new(PayloadStatusEnum::Syncing, None);
        let encoded = encode_new_payload_with_witness_response(&status, None);
        assert_eq!(encoded[0], 2u8); // SYNCING
    }

    #[test]
    fn test_encode_valid_response_with_witness() {
        let status = PayloadStatus::new(
            PayloadStatusEnum::Valid,
            Some(B256::repeat_byte(0x11)),
        );
        let witness = ExecutionWitness {
            state: vec![alloy_primitives::Bytes::from_static(b"node1")],
            codes: vec![],
            keys: vec![],
            headers: vec![],
        };
        let encoded = encode_new_payload_with_witness_response(&status, Some(&witness));
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], 0u8); // VALID
    }
}
