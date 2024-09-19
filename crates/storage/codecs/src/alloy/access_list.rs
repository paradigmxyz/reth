use crate::Compact;
use alloc::vec::Vec;
use alloy_eips::eip2930::{AccessList, AccessListItem};
use alloy_primitives::Address;

/// Implement `Compact` for `AccessListItem` and `AccessList`.
impl Compact for AccessListItem {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut buffer = Vec::new();
        self.address.to_compact(&mut buffer);
        self.storage_keys.specialized_to_compact(&mut buffer);
        buf.put(&buffer[..]);
        buffer.len()
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (address, new_buf) = Address::from_compact(buf, buf.len());
        buf = new_buf;
        let (storage_keys, new_buf) = Vec::specialized_from_compact(buf, buf.len());
        buf = new_buf;
        let access_list_item = Self { address, storage_keys };
        (access_list_item, buf)
    }
}

impl Compact for AccessList {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut buffer = Vec::new();
        self.0.to_compact(&mut buffer);
        buf.put(&buffer[..]);
        buffer.len()
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (access_list_items, new_buf) = Vec::from_compact(buf, buf.len());
        buf = new_buf;
        let access_list = Self(access_list_items);
        (access_list, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;
    use serde::Deserialize;

    proptest! {
        #[test]
        fn test_roundtrip_compact_access_list_item(access_list_item in arb::<AccessListItem>()) {
            let mut compacted_access_list_item = Vec::<u8>::new();
            let len = access_list_item.to_compact(&mut compacted_access_list_item);

            let (decoded_access_list_item, _) = AccessListItem::from_compact(&compacted_access_list_item, len);
            assert_eq!(access_list_item, decoded_access_list_item);
        }
    }

    proptest! {
        #[test]
        fn test_roundtrip_compact_access_list(access_list in arb::<AccessList>()) {
            let mut compacted_access_list = Vec::<u8>::new();
            let len = access_list.to_compact(&mut compacted_access_list);

            let (decoded_access_list, _) = AccessList::from_compact(&compacted_access_list, len);
            assert_eq!(access_list, decoded_access_list);
        }
    }

    #[derive(Deserialize)]
    struct CompactAccessListTestVector {
        access_list: AccessList,
        encoded_bytes: Bytes,
    }

    #[test]
    fn test_compact_access_list_codec() {
        let test_vectors: Vec<CompactAccessListTestVector> =
            serde_json::from_str(include_str!("../../testdata/access_list_compact.json"))
                .expect("Failed to parse test vectors");

        for test_vector in test_vectors {
            let mut buf = Vec::<u8>::new();
            let len = test_vector.access_list.clone().to_compact(&mut buf);
            assert_eq!(test_vector.encoded_bytes.0, buf);

            let (decoded, _) = AccessList::from_compact(&test_vector.encoded_bytes, len);
            assert_eq!(test_vector.access_list, decoded);
        }
    }
}
