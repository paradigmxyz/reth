use alloy_primitives::{b256, B256};
pub use poseidon_bn254::{hash_with_domain, Fr, PrimeField};

/// The Poseidon hash of the empty string `""`.
pub const POSEIDON_EMPTY: B256 =
    b256!("2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864");

/// The root hash of an empty binary Merle Patricia trie.
pub const EMPTY_ROOT_HASH: B256 = B256::ZERO;

/// Type that is used to represent a field element in binary representation.
pub type FieldElementBytes = <Fr as PrimeField>::Repr;

/// The number of bytes in the binary representation of a field element.
pub const FIELD_ELEMENT_REPR_BYTES: usize = core::mem::size_of::<FieldElementBytes>();

// Half the number of bytes in the binary representation of a field element.
const HALF_FIELD_ELEMENT_REPR_BYTES: usize = FIELD_ELEMENT_REPR_BYTES / 2;

/// The domain multiplier per field element.
pub const DOMAIN_MULTIPLIER_PER_FIELD_ELEMENT: u64 = 256;

/// The domain for hashing two field elements.
pub const DOMAIN_TWO_FIELD_ELEMENTS: Fr =
    Fr::from_raw([DOMAIN_MULTIPLIER_PER_FIELD_ELEMENT * 2, 0, 0, 0]);

/// Hash two field elements using poseidon.
pub fn hash(element_1: Fr, element_2: Fr) -> Fr {
    hash_with_domain(&[element_1, element_2], DOMAIN_TWO_FIELD_ELEMENTS)
}

/// Poseidon code hash
pub fn hash_code<T: AsRef<[u8]>>(code: T) -> B256 {
    poseidon_bn254::hash_code(code.as_ref()).into()
}

/// Split and transform input be bytes into two field elements and hash using poseidon.
///
/// # Panics
///
/// This function will panic if more than 32 bytes are provided as input.
pub fn split_and_hash_be_bytes<T: AsRef<[u8]>>(bytes: T) -> Fr {
    debug_assert!(
        bytes.as_ref().len() <= FIELD_ELEMENT_REPR_BYTES,
        "bytes length should be less than or equal to field element bytes"
    );
    let (bytes_lo, bytes_hi) = split_and_parse_field_elements(bytes.as_ref());
    hash(bytes_lo, bytes_hi)
}

/// Parse input bytes into two field elements which represent the lower bytes and the upper bytes.
///
/// # Panics
///
/// This function will panic if more than 32 bytes are provided as input.
fn split_and_parse_field_elements(bytes: &[u8]) -> (Fr, Fr) {
    debug_assert!(
        bytes.len() <= FIELD_ELEMENT_REPR_BYTES,
        "bytes length should be less than or equal to field element bytes"
    );
    let mut bytes_lo = FieldElementBytes::default();
    let mut bytes_hi = FieldElementBytes::default();

    if bytes.len() > (HALF_FIELD_ELEMENT_REPR_BYTES) {
        bytes_lo[HALF_FIELD_ELEMENT_REPR_BYTES..]
            .copy_from_slice(&bytes[..HALF_FIELD_ELEMENT_REPR_BYTES]);
        bytes_hi[HALF_FIELD_ELEMENT_REPR_BYTES..bytes.len()]
            .copy_from_slice(&bytes[HALF_FIELD_ELEMENT_REPR_BYTES..]);
    } else {
        bytes_lo[HALF_FIELD_ELEMENT_REPR_BYTES..(HALF_FIELD_ELEMENT_REPR_BYTES + bytes.len())]
            .copy_from_slice(bytes)
    }

    let bytes_lo = field_element_from_be_bytes(bytes_lo);
    let bytes_hi = field_element_from_be_bytes(bytes_hi);
    (bytes_lo, bytes_hi)
}

/// Parses a field element from big endian bytes.
///
/// # Panics
///
/// This function will panic if the bytes are not a valid field element.
pub fn field_element_from_be_bytes(mut bytes: FieldElementBytes) -> Fr {
    bytes.reverse();
    Fr::from_repr_vartime(bytes).expect("valid field element")
}
