//!  [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930): Access List types

#[cfg(test)]
mod tests {
    use alloy_eips::eip2930::{AccessList, AccessListItem};
    use alloy_primitives::{Address, B256};
    use alloy_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;
    use reth_codecs::{add_arbitrary_tests, Compact};
    use serde::{Deserialize, Serialize};

    /// This type is kept for compatibility tests after the codec support was added to alloy-eips
    /// AccessList type natively
    #[derive(
        Clone,
        Debug,
        PartialEq,
        Eq,
        Hash,
        Default,
        RlpDecodableWrapper,
        RlpEncodableWrapper,
        Serialize,
        Deserialize,
        Compact,
    )]
    #[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
    #[add_arbitrary_tests(compact, rlp)]
    struct RethAccessList(Vec<RethAccessListItem>);

    impl PartialEq<AccessList> for RethAccessList {
        fn eq(&self, other: &AccessList) -> bool {
            self.0.iter().zip(other.iter()).all(|(a, b)| a == b)
        }
    }

    // This
    #[derive(
        Clone,
        Debug,
        PartialEq,
        Eq,
        Hash,
        Default,
        RlpDecodable,
        RlpEncodable,
        Serialize,
        Deserialize,
        Compact,
    )]
    #[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
    #[add_arbitrary_tests(compact, rlp)]
    #[serde(rename_all = "camelCase")]
    struct RethAccessListItem {
        /// Account address that would be loaded at the start of execution
        address: Address,
        /// The storage keys to be loaded at the start of execution.
        ///
        /// Each key is a 32-byte value representing a specific storage slot.
        storage_keys: Vec<B256>,
    }

    impl PartialEq<AccessListItem> for RethAccessListItem {
        fn eq(&self, other: &AccessListItem) -> bool {
            self.address == other.address && self.storage_keys == other.storage_keys
        }
    }

    proptest!(
        #[test]
        fn test_roundtrip_accesslist_compat(access_list in arb::<RethAccessList>()) {
            // Convert access_list to buffer and then create alloy_access_list from buffer and
            // compare
            let mut compacted_reth_access_list = Vec::<u8>::new();
            let len = access_list.to_compact(&mut compacted_reth_access_list);

            // decode the compacted buffer to AccessList
            let alloy_access_list = AccessList::from_compact(&compacted_reth_access_list, len).0;
            assert_eq!(access_list, alloy_access_list);

            let mut compacted_alloy_access_list = Vec::<u8>::new();
            let alloy_len = alloy_access_list.to_compact(&mut compacted_alloy_access_list);
            assert_eq!(len, alloy_len);
            assert_eq!(compacted_reth_access_list, compacted_alloy_access_list);
        }
    );
}
