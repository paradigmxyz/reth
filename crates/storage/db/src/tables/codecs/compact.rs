use crate::{
    models::client_version::ClientVersion,
    table::{Compress, Decompress},
    tables::models::*,
};
use reth_codecs::{main_codec, Compact};
use reth_primitives::{stage::StageCheckpoint, trie::*, *};

/// Implements compression for Compact type.
macro_rules! impl_compression_for_compact {
    ($($name:tt),+) => {
        $(
            impl Compress for $name {
                type Compressed = Vec<u8>;

                fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
                    let _ = Compact::to_compact(self, buf);
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
    StoredBranchNode,
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
                    let _  = Compact::to_compact(self, buf);
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

/// Adds wrapper structs for some primitive types so they can use StructFlags from Compact, when
/// used as pure table values.
macro_rules! add_wrapper_struct {
    ($(($name:tt, $wrapper:tt)),+) => {
        $(
            /// Wrapper struct so it can use StructFlags from Compact, when used as pure table values.
            #[main_codec]
            #[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    use crate::{
        codecs::{
            compact::{CompactClientVersion, CompactU64},
            CompactU256,
        },
        models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
    };
    use reth_primitives::{
        stage::{
            AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint,
            ExecutionCheckpoint, HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint,
            StageUnitCheckpoint, StorageHashingCheckpoint,
        },
        Account, Header, PruneCheckpoint, PruneMode, PruneSegment, Receipt, ReceiptWithBloom,
        SealedHeader, TxEip1559, TxEip2930, TxEip4844, TxLegacy, Withdrawals,
    };

    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility
    #[test]
    fn test_ensure_backwards_compatibility() {
        #[cfg(not(feature = "optimism"))]
        {
            assert_eq!(Account::bitflag_encoded_bytes(), 2);
            assert_eq!(AccountHashingCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(CheckpointBlockRange::bitflag_encoded_bytes(), 1);
            assert_eq!(CompactClientVersion::bitflag_encoded_bytes(), 0);
            assert_eq!(CompactU256::bitflag_encoded_bytes(), 1);
            assert_eq!(CompactU64::bitflag_encoded_bytes(), 1);
            assert_eq!(EntitiesCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(ExecutionCheckpoint::bitflag_encoded_bytes(), 0);
            assert_eq!(Header::bitflag_encoded_bytes(), 4);
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
            assert_eq!(TxEip1559::bitflag_encoded_bytes(), 4);
            assert_eq!(TxEip2930::bitflag_encoded_bytes(), 3);
            assert_eq!(TxEip4844::bitflag_encoded_bytes(), 5);
            assert_eq!(TxLegacy::bitflag_encoded_bytes(), 3);
            assert_eq!(Withdrawals::bitflag_encoded_bytes(), 0);
        }

        #[cfg(feature = "optimism")]
        {
            assert_eq!(Account::bitflag_encoded_bytes(), 2);
            assert_eq!(AccountHashingCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(CheckpointBlockRange::bitflag_encoded_bytes(), 1);
            assert_eq!(CompactClientVersion::bitflag_encoded_bytes(), 0);
            assert_eq!(CompactU256::bitflag_encoded_bytes(), 1);
            assert_eq!(CompactU64::bitflag_encoded_bytes(), 1);
            assert_eq!(EntitiesCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(ExecutionCheckpoint::bitflag_encoded_bytes(), 0);
            assert_eq!(Header::bitflag_encoded_bytes(), 4);
            assert_eq!(HeadersCheckpoint::bitflag_encoded_bytes(), 0);
            assert_eq!(IndexHistoryCheckpoint::bitflag_encoded_bytes(), 0);
            assert_eq!(PruneCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(PruneMode::bitflag_encoded_bytes(), 1);
            assert_eq!(PruneSegment::bitflag_encoded_bytes(), 1);
            assert_eq!(Receipt::bitflag_encoded_bytes(), 2);
            assert_eq!(ReceiptWithBloom::bitflag_encoded_bytes(), 0);
            assert_eq!(SealedHeader::bitflag_encoded_bytes(), 0);
            assert_eq!(StageCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(StageUnitCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(StoredBlockBodyIndices::bitflag_encoded_bytes(), 1);
            assert_eq!(StoredBlockOmmers::bitflag_encoded_bytes(), 0);
            assert_eq!(StoredBlockWithdrawals::bitflag_encoded_bytes(), 0);
            assert_eq!(StorageHashingCheckpoint::bitflag_encoded_bytes(), 1);
            assert_eq!(TxEip1559::bitflag_encoded_bytes(), 4);
            assert_eq!(TxEip2930::bitflag_encoded_bytes(), 3);
            assert_eq!(TxEip4844::bitflag_encoded_bytes(), 5);
            assert_eq!(TxLegacy::bitflag_encoded_bytes(), 3);
            assert_eq!(Withdrawals::bitflag_encoded_bytes(), 0);
        }
    }
}
