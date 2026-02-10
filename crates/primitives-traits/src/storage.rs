use alloy_primitives::{B256, U256};

/// Trait for `DupSort` table values that contain a subkey.
///
/// This trait allows extracting the subkey from a value during database iteration,
/// enabling proper range queries and filtering on `DupSort` tables.
pub trait ValueWithSubKey {
    /// The type of the subkey.
    type SubKey;

    /// Extract the subkey from the value.
    fn get_subkey(&self) -> Self::SubKey;
}

/// Account storage entry.
///
/// `key` is the subkey when used as a value in the `StorageChangeSets` table.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct StorageEntry {
    /// Storage key.
    pub key: B256,
    /// Value on storage key.
    pub value: U256,
}

impl StorageEntry {
    /// Create a new `StorageEntry` with given key and value.
    pub const fn new(key: B256, value: U256) -> Self {
        Self { key, value }
    }
}

impl ValueWithSubKey for StorageEntry {
    type SubKey = B256;

    fn get_subkey(&self) -> Self::SubKey {
        self.key
    }
}

impl From<(B256, U256)> for StorageEntry {
    fn from((key, value): (B256, U256)) -> Self {
        Self { key, value }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, keccak256, B256, U256};

    #[test]
    fn plain_to_hashed_matches_keccak256() {
        let slot = B256::repeat_byte(0xAB);
        let key = StorageSlotKey::Plain(slot);
        assert_eq!(key.to_hashed(), keccak256(slot));
    }

    #[test]
    fn hashed_to_hashed_is_noop() {
        let h = keccak256(B256::repeat_byte(0x01));
        let key = StorageSlotKey::Hashed(h);
        assert_eq!(key.to_hashed(), h, "to_hashed on Hashed must be identity");
    }

    #[test]
    fn plain_to_hashed_roundtrip_as_b256() {
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000005");
        let plain = StorageSlotKey::Plain(slot);
        let hashed_val = plain.to_hashed();
        assert_eq!(hashed_val, keccak256(slot));
        let hashed_key = StorageSlotKey::Hashed(hashed_val);
        assert_eq!(hashed_key.as_b256(), hashed_val);
        assert_eq!(hashed_key.to_hashed(), hashed_val, "double-hash must not occur");
    }

    #[test]
    fn from_raw_false_is_plain() {
        let b = B256::repeat_byte(0x42);
        let key = StorageSlotKey::from_raw(b, false);
        assert!(key.is_plain());
        assert!(!key.is_hashed());
        assert_eq!(key.as_b256(), b);
    }

    #[test]
    fn from_raw_true_is_hashed() {
        let b = B256::repeat_byte(0x42);
        let key = StorageSlotKey::from_raw(b, true);
        assert!(key.is_hashed());
        assert!(!key.is_plain());
        assert_eq!(key.as_b256(), b);
    }

    #[test]
    fn from_u256_zero() {
        let key = StorageSlotKey::from_u256(U256::ZERO);
        assert!(key.is_plain());
        assert_eq!(key.as_b256(), B256::ZERO);
    }

    #[test]
    fn from_u256_max() {
        let key = StorageSlotKey::from_u256(U256::MAX);
        assert!(key.is_plain());
        assert_eq!(key.as_b256(), B256::from([0xFF; 32]));
    }

    #[test]
    fn from_u256_one() {
        let key = StorageSlotKey::from_u256(U256::from(1));
        let mut expected = [0u8; 32];
        expected[31] = 1;
        assert_eq!(key.as_b256(), B256::from(expected));
    }

    #[test]
    fn from_u256_preserves_big_endian() {
        let val = U256::from(0xDEAD_BEEFu64);
        let key = StorageSlotKey::from_u256(val);
        let bytes = key.as_b256();
        assert_eq!(bytes[28], 0xDE);
        assert_eq!(bytes[29], 0xAD);
        assert_eq!(bytes[30], 0xBE);
        assert_eq!(bytes[31], 0xEF);
    }

    #[test]
    fn default_is_plain_zero() {
        let key = StorageSlotKey::default();
        assert!(key.is_plain());
        assert_eq!(key.as_b256(), B256::ZERO);
    }

    #[test]
    fn eq_same_variant_same_bytes() {
        let b = B256::repeat_byte(0x11);
        assert_eq!(StorageSlotKey::Plain(b), StorageSlotKey::Plain(b));
        assert_eq!(StorageSlotKey::Hashed(b), StorageSlotKey::Hashed(b));
    }

    #[test]
    fn ne_different_variant_same_bytes() {
        let b = B256::repeat_byte(0x22);
        assert_ne!(StorageSlotKey::Plain(b), StorageSlotKey::Hashed(b));
    }

    #[test]
    fn ord_plain_before_hashed_derived() {
        let b = B256::repeat_byte(0x01);
        assert!(StorageSlotKey::Plain(b) < StorageSlotKey::Hashed(b),
            "derived Ord: Plain (discriminant 0) < Hashed (discriminant 1)");
    }

    #[test]
    fn ord_within_variant_by_bytes() {
        let lo = B256::repeat_byte(0x01);
        let hi = B256::repeat_byte(0x02);
        assert!(StorageSlotKey::Plain(lo) < StorageSlotKey::Plain(hi));
        assert!(StorageSlotKey::Hashed(lo) < StorageSlotKey::Hashed(hi));
    }

    #[test]
    fn hash_consistency_with_eq() {
        use core::hash::{Hash, Hasher};
        let b = B256::repeat_byte(0x33);
        let k1 = StorageSlotKey::Plain(b);
        let k2 = StorageSlotKey::Plain(b);
        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        k1.hash(&mut h1);
        k2.hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn hash_differs_across_variants() {
        use core::hash::{Hash, Hasher};
        let b = B256::repeat_byte(0x44);
        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        StorageSlotKey::Plain(b).hash(&mut h1);
        StorageSlotKey::Hashed(b).hash(&mut h2);
        assert_ne!(h1.finish(), h2.finish(), "different variants must hash differently");
    }

    #[test]
    fn to_changeset_key_plain_v1() {
        let b = B256::repeat_byte(0x55);
        let key = StorageSlotKey::Plain(b);
        assert_eq!(key.to_changeset_key(false), b);
    }

    #[test]
    fn to_changeset_key_plain_v2() {
        let b = B256::repeat_byte(0x55);
        let key = StorageSlotKey::Plain(b);
        assert_eq!(key.to_changeset_key(true), keccak256(b));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "to_changeset_key called on already-hashed key")]
    fn to_changeset_key_hashed_panics_debug() {
        let h = keccak256(B256::repeat_byte(0x66));
        StorageSlotKey::Hashed(h).to_changeset_key(false);
    }

    #[test]
    fn to_changeset_returns_tagged_key() {
        let b = B256::repeat_byte(0x77);
        let plain = StorageSlotKey::Plain(b);
        let cs_v1 = plain.to_changeset(false);
        assert!(cs_v1.is_plain());
        assert_eq!(cs_v1.as_b256(), b);

        let cs_v2 = plain.to_changeset(true);
        assert!(cs_v2.is_hashed());
        assert_eq!(cs_v2.as_b256(), keccak256(b));
    }

    #[test]
    fn into_b256_from_storage_slot_key() {
        let b = B256::repeat_byte(0x88);
        let key = StorageSlotKey::Plain(b);
        let raw: B256 = key.into();
        assert_eq!(raw, b);
    }

    #[test]
    fn storage_entry_slot_key_tagging() {
        let entry = StorageEntry { key: B256::repeat_byte(0x99), value: U256::from(42) };
        let plain = entry.slot_key(false);
        assert!(plain.is_plain());
        assert_eq!(plain.as_b256(), entry.key);

        let hashed = entry.slot_key(true);
        assert!(hashed.is_hashed());
        assert_eq!(hashed.as_b256(), entry.key);
    }

    #[test]
    fn storage_entry_from_tuple() {
        let k = B256::repeat_byte(0xAA);
        let v = U256::from(100);
        let entry = StorageEntry::from((k, v));
        assert_eq!(entry.key, k);
        assert_eq!(entry.value, v);
    }

    #[test]
    fn storage_entry_get_subkey() {
        let entry = StorageEntry::new(B256::repeat_byte(0xBB), U256::from(200));
        assert_eq!(entry.get_subkey(), entry.key);
    }

    #[test]
    fn compact_roundtrip() {
        let entry = StorageEntry::new(B256::repeat_byte(0xCC), U256::from(12345));
        let mut buf = Vec::new();
        let len = reth_codecs::Compact::to_compact(&entry, &mut buf);
        let (decoded, rest) = StorageEntry::from_compact(&buf, len);
        assert_eq!(decoded.key, entry.key);
        assert_eq!(decoded.value, entry.value);
        assert!(rest.is_empty());
    }

    #[test]
    fn compact_roundtrip_zero_value() {
        let entry = StorageEntry::new(B256::ZERO, U256::ZERO);
        let mut buf = Vec::new();
        let len = reth_codecs::Compact::to_compact(&entry, &mut buf);
        let (decoded, _) = StorageEntry::from_compact(&buf, len);
        assert_eq!(decoded.key, B256::ZERO);
        assert_eq!(decoded.value, U256::ZERO);
    }

    #[test]
    fn compact_roundtrip_max_value() {
        let entry = StorageEntry::new(B256::from([0xFF; 32]), U256::MAX);
        let mut buf = Vec::new();
        let len = reth_codecs::Compact::to_compact(&entry, &mut buf);
        let (decoded, _) = StorageEntry::from_compact(&buf, len);
        assert_eq!(decoded.key, B256::from([0xFF; 32]));
        assert_eq!(decoded.value, U256::MAX);
    }

    #[test]
    #[should_panic]
    fn compact_from_compact_panics_on_short_buffer() {
        let buf = [0u8; 16];
        let _ = StorageEntry::from_compact(&buf, 48);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn to_changeset_key_hashed_true_release() {
        let h = keccak256(B256::repeat_byte(0x99));
        let result = StorageSlotKey::Hashed(h).to_changeset_key(true);
        assert_eq!(result, h, "Hashed(h).to_changeset_key(true) accidentally correct: to_hashed is no-op");
    }

    #[test]
    fn to_changeset_key_matrix() {
        let slot = B256::repeat_byte(0xAB);
        let plain = StorageSlotKey::Plain(slot);

        assert_eq!(plain.to_changeset_key(false), slot, "Plain v1: raw key");
        assert_eq!(plain.to_changeset_key(true), keccak256(slot), "Plain v2: hashed");

        let cs_v1 = plain.to_changeset(false);
        assert!(cs_v1.is_plain());
        assert_eq!(cs_v1.as_b256(), slot);

        let cs_v2 = plain.to_changeset(true);
        assert!(cs_v2.is_hashed());
        assert_eq!(cs_v2.as_b256(), keccak256(slot));
    }

    #[test]
    fn from_u256_to_changeset_key_roundtrip() {
        let slot = U256::from(7u64);
        let key = StorageSlotKey::from_u256(slot);
        let changeset_key = key.to_changeset_key(true);
        let expected = keccak256(B256::from(slot.to_be_bytes()));
        assert_eq!(changeset_key, expected, "write-path roundtrip: from_u256 -> to_changeset_key(true)");
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageEntry {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        buf.put_slice(&self.key[..]);
        self.value.to_compact(buf) + 32
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let key = B256::from_slice(&buf[..32]);
        let (value, out) = U256::from_compact(&buf[32..], len - 32);
        (Self { key, value }, out)
    }
}
