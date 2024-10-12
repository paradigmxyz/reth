//! Implements data structures specific to the database

use crate::{
    table::{Compress, Decode, Decompress, Encode},
    DatabaseError,
};
use alloy_genesis::GenesisAccount;
use alloy_primitives::{Address, Bytes, Log, B256, U256};
use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::{
    Account, Bytecode, Header, Receipt, Requests, StorageEntry, TransactionSignedNoHash, TxType,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::StageCheckpoint;
use reth_trie_common::{StoredNibbles, StoredNibblesSubKey, *};
use serde::{Deserialize, Serialize};

pub mod accounts;
pub mod blocks;
pub mod integer_list;
pub mod sharded_key;
pub mod storage_sharded_key;

pub use accounts::*;
pub use blocks::*;
pub use reth_db_models::{
    AccountBeforeTx, ClientVersion, StoredBlockBodyIndices, StoredBlockWithdrawals,
};
pub use sharded_key::ShardedKey;

/// Macro that implements [`Encode`] and [`Decode`] for uint types.
macro_rules! impl_uints {
    ($($name:tt),+) => {
        $(
            impl Encode for $name {
                type Encoded = [u8; std::mem::size_of::<$name>()];

                fn encode(self) -> Self::Encoded {
                    self.to_be_bytes()
                }
            }

            impl Decode for $name {
                fn decode(value: &[u8]) -> Result<Self, $crate::DatabaseError> {
                    Ok(
                        $name::from_be_bytes(
                            value.try_into().map_err(|_| $crate::DatabaseError::Decode)?
                        )
                    )
                }
            }
        )+
    };
}

impl_uints!(u64, u32, u16, u8);

impl Encode for Vec<u8> {
    type Encoded = Self;

    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Vec<u8> {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(value.to_vec())
    }

    fn decode_owned(value: Vec<u8>) -> Result<Self, DatabaseError> {
        Ok(value)
    }
}

impl Encode for Address {
    type Encoded = [u8; 20];

    fn encode(self) -> Self::Encoded {
        self.0 .0
    }
}

impl Decode for Address {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::from_slice(value))
    }
}

impl Encode for B256 {
    type Encoded = [u8; 32];

    fn encode(self) -> Self::Encoded {
        self.0
    }
}

impl Decode for B256 {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::new(value.try_into().map_err(|_| DatabaseError::Decode)?))
    }
}

impl Encode for String {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        self.into_bytes()
    }
}

impl Decode for String {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Self::decode_owned(value.to_vec())
    }

    fn decode_owned(value: Vec<u8>) -> Result<Self, DatabaseError> {
        Self::from_utf8(value).map_err(|_| DatabaseError::Decode)
    }
}

impl Encode for StoredNibbles {
    type Encoded = Vec<u8>;

    // Delegate to the Compact implementation
    fn encode(self) -> Self::Encoded {
        // NOTE: This used to be `to_compact`, but all it does is append the bytes to the buffer,
        // so we can just use the implementation of `Into<Vec<u8>>` to reuse the buffer.
        self.0.into()
    }
}

impl Decode for StoredNibbles {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::from_compact(value, value.len()).0)
    }
}

impl Encode for StoredNibblesSubKey {
    type Encoded = Vec<u8>;

    // Delegate to the Compact implementation
    fn encode(self) -> Self::Encoded {
        let mut buf = Vec::with_capacity(65);
        self.to_compact(&mut buf);
        buf
    }
}

impl Decode for StoredNibblesSubKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::from_compact(value, value.len()).0)
    }
}

impl Encode for PruneSegment {
    type Encoded = [u8; 1];

    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8];
        self.to_compact(&mut buf.as_mut());
        buf
    }
}

impl Decode for PruneSegment {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::from_compact(value, value.len()).0)
    }
}

impl Encode for ClientVersion {
    type Encoded = Vec<u8>;

    // Delegate to the Compact implementation
    fn encode(self) -> Self::Encoded {
        let mut buf = vec![];
        self.to_compact(&mut buf);
        buf
    }
}

impl Decode for ClientVersion {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        Ok(Self::from_compact(value, value.len()).0)
    }
}

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
                fn decompress(value: &[u8]) -> Result<$name, $crate::DatabaseError> {
                    let (obj, _) = Compact::from_compact(value, value.len());
                    Ok(obj)
                }
            }
        )+
    };
}

impl_compression_for_compact!(
    Bytes,
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
            impl Compress for $name {
                type Compressed = Vec<u8>;

                fn uncompressable_ref(&self) -> Option<&[u8]> {
                    Some(self.as_ref())
                }

                fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
                    let _  = Compact::to_compact(&self, buf);
                }
            }

            impl Decompress for $name {
                fn decompress(value: &[u8]) -> Result<$name, $crate::DatabaseError> {
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
        use reth_primitives::{Account, Receipt, ReceiptWithBloom, Withdrawals};
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
