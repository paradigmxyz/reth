//! Implements data structures specific to the database
use crate::table::{Compress, Decompress};
use alloy_genesis::GenesisAccount;
use alloy_primitives::{Address, Log, B256, U256};
use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{
    Account, Bytecode, Header, Receipt, Requests, SealedHeader, StorageEntry,
    TransactionSignedNoHash, TxType,
};
use reth_prune_types::PruneCheckpoint;
use reth_stages_types::StageCheckpoint;
use reth_trie_common::{StoredNibbles, StoredNibblesSubKey, *};
use serde::{Deserialize, Serialize};

pub mod blocks;
pub mod integer_list;

pub use blocks::*;
pub use reth_db_models::{
    sharded_key, storage_sharded_key, AccountBeforeTx, AddressStorageKey, BlockNumberAddress,
    ClientVersion, ShardedKey, StorageShardedKey, StoredBlockBodyIndices, StoredBlockWithdrawals,
};

/// Implements compression for Compact type.
macro_rules! impl_compression_for_compact {
    ($($name:tt),+) => {
        $(
            impl Compress for $name {
                type Compressed = Vec<u8>;

                fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
                    let _ = Compact::to_compact(&self, buf);
                }
            }

            impl Decompress for $name {
                fn decompress<B: AsRef<[u8]>>(value: B) -> Result<$name, $crate::DatabaseError> {
                    let value = value.as_ref();
                    let (obj, _) = Compact::from_compact(&value, value.len());
                    Ok(obj)
                }
            }
        )+
    };
}

impl_compression_for_compact!(
    SealedHeader,
    Header,
    Account,
    Log,
    Receipt,
    TxType,
    StorageEntry,
    BranchNodeCompact,
    StoredNibbles,
    StoredNibblesSubKey,
    StorageTrieEntry,
    StoredBlockBodyIndices,
    StoredBlockOmmers,
    StoredBlockWithdrawals,
    Bytecode,
    AccountBeforeTx,
    TransactionSignedNoHash,
    CompactU256,
    StageCheckpoint,
    PruneCheckpoint,
    ClientVersion,
    Requests,
    // Non-DB
    GenesisAccount
);

macro_rules! impl_compression_fixed_compact {
    ($($name:tt),+) => {
        $(
            impl Compress for $name
            {
                type Compressed = Vec<u8>;

                fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
                    let _  = Compact::to_compact(&self, buf);
                }

                fn uncompressable_ref(&self) -> Option<&[u8]> {
                    Some(self.as_ref())
                }
            }

            impl Decompress for $name
            {
                fn decompress<B: AsRef<[u8]>>(value: B) -> Result<$name, $crate::DatabaseError> {
                    let value = value.as_ref();
                    let (obj, _) = Compact::from_compact(&value, value.len());
                    Ok(obj)
                }
            }

        )+
    };
}

impl_compression_fixed_compact!(B256, Address);

/// Adds wrapper structs for some primitive types so they can use `StructFlags` from Compact, when
/// used as pure table values.
macro_rules! add_wrapper_struct {
    ($(($name:tt, $wrapper:tt)),+) => {
        $(
            /// Wrapper struct so it can use StructFlags from Compact, when used as pure table values.
            #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Compact)]
            #[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
            #[add_arbitrary_tests(compact)]
            pub struct $wrapper(pub $name);

            impl From<$name> for $wrapper {
                fn from(value: $name) -> Self {
                    $wrapper(value)
                }
            }

            impl From<$wrapper> for $name {
                fn from(value: $wrapper) -> Self {
                    value.0
                }
            }

            impl std::ops::Deref for $wrapper {
                type Target = $name;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

        )+
    };
}

add_wrapper_struct!((U256, CompactU256));
add_wrapper_struct!((u64, CompactU64));
add_wrapper_struct!((ClientVersion, CompactClientVersion));

#[cfg(test)]
mod tests {
    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility
    #[cfg(not(feature = "optimism"))]
    #[test]
    fn test_ensure_backwards_compatibility() {
        use super::*;
        use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};
        use reth_primitives::{Account, Receipt, ReceiptWithBloom, SealedHeader, Withdrawals};
        use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
        use reth_stages_types::{
            AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint,
            ExecutionCheckpoint, HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint,
            StageUnitCheckpoint, StorageHashingCheckpoint,
        };
        assert_eq!(Account::bitflag_encoded_bytes(), 2);
        assert_eq!(AccountHashingCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(CheckpointBlockRange::bitflag_encoded_bytes(), 1);
        assert_eq!(CompactClientVersion::bitflag_encoded_bytes(), 0);
        assert_eq!(CompactU256::bitflag_encoded_bytes(), 1);
        assert_eq!(CompactU64::bitflag_encoded_bytes(), 1);
        assert_eq!(EntitiesCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(ExecutionCheckpoint::bitflag_encoded_bytes(), 0);
        assert_eq!(HeadersCheckpoint::bitflag_encoded_bytes(), 0);
        assert_eq!(IndexHistoryCheckpoint::bitflag_encoded_bytes(), 0);
        assert_eq!(PruneCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(PruneMode::bitflag_encoded_bytes(), 1);
        assert_eq!(PruneSegment::bitflag_encoded_bytes(), 1);
        assert_eq!(Receipt::bitflag_encoded_bytes(), 1);
        assert_eq!(ReceiptWithBloom::bitflag_encoded_bytes(), 0);
        assert_eq!(SealedHeader::bitflag_encoded_bytes(), 0);
        assert_eq!(StageCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(StageUnitCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(StoredBlockBodyIndices::bitflag_encoded_bytes(), 1);
        assert_eq!(StoredBlockOmmers::bitflag_encoded_bytes(), 0);
        assert_eq!(StoredBlockWithdrawals::bitflag_encoded_bytes(), 0);
        assert_eq!(StorageHashingCheckpoint::bitflag_encoded_bytes(), 1);
        assert_eq!(Withdrawals::bitflag_encoded_bytes(), 0);

        validate_bitflag_backwards_compat!(Account, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(AccountHashingCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(CheckpointBlockRange, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(CompactClientVersion, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(CompactU256, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(CompactU64, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(EntitiesCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(ExecutionCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(HeadersCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(IndexHistoryCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(PruneCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(PruneMode, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(PruneSegment, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(Receipt, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(ReceiptWithBloom, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(SealedHeader, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StageCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(StageUnitCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockBodyIndices, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockOmmers, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockWithdrawals, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StorageHashingCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(Withdrawals, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(Requests, UnusedBits::Zero);
    }
}
