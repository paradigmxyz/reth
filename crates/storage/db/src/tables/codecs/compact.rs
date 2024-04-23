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
    ClientVersion
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
    use reth_primitives::{stage::{AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint, HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint, StageUnitCheckpoint, StorageHashingCheckpoint}, Account, Header, PruneCheckpoint, PruneMode, PruneSegment, Receipt, ReceiptWithBloom, SealedHeader, TxEip1559, TxEip2930, TxEip4844, TxLegacy, Withdrawal, Withdrawals};
    use crate::{codecs::{compact::{CompactClientVersion, CompactU64}, CompactU256}, models::{StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals}};
    
    #[test]
    fn test_ensure_backwards_compatibility() {
        assert!(Account::bitflag_encoded_bytes() == 2);
        assert!(Account::bitflag_unused_bits() == 5);
        assert!(AccountHashingCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(AccountHashingCheckpoint::bitflag_unused_bits() == 7);
        assert!(CheckpointBlockRange::bitflag_encoded_bytes() == 1);
        assert!(CheckpointBlockRange::bitflag_unused_bits() == 0);
        assert!(CompactClientVersion::bitflag_encoded_bytes() == 0);
        assert!(CompactClientVersion::bitflag_unused_bits() == 0);
        assert!(CompactU256::bitflag_encoded_bytes() == 1);
        assert!(CompactU256::bitflag_unused_bits() == 2);
        assert!(CompactU64::bitflag_encoded_bytes() == 1);
        assert!(CompactU64::bitflag_unused_bits() == 4);
        assert!(EntitiesCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(EntitiesCheckpoint::bitflag_unused_bits() == 0);
        assert!(ExecutionCheckpoint::bitflag_encoded_bytes() == 0);
        assert!(ExecutionCheckpoint::bitflag_unused_bits() == 0);
        assert!(Header::bitflag_encoded_bytes() == 4);
        assert!(Header::bitflag_unused_bits() == 1);
        assert!(HeadersCheckpoint::bitflag_encoded_bytes() == 0);
        assert!(HeadersCheckpoint::bitflag_unused_bits() == 0);
        assert!(IndexHistoryCheckpoint::bitflag_encoded_bytes() == 0);
        assert!(IndexHistoryCheckpoint::bitflag_unused_bits() == 0);
        assert!(PruneCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(PruneCheckpoint::bitflag_unused_bits() == 6);
        assert!(PruneMode::bitflag_encoded_bytes() == 1);
        assert!(PruneMode::bitflag_unused_bits() == 0);
        assert!(PruneSegment::bitflag_encoded_bytes() == 1);
        assert!(PruneSegment::bitflag_unused_bits() == 0);
        assert!(Receipt::bitflag_encoded_bytes() == 1);
        assert!(Receipt::bitflag_unused_bits() == 0);
        assert!(ReceiptWithBloom::bitflag_encoded_bytes() == 0);
        assert!(ReceiptWithBloom::bitflag_unused_bits() == 0);
        assert!(SealedHeader::bitflag_encoded_bytes() == 0);
        assert!(SealedHeader::bitflag_unused_bits() == 0);
        assert!(StageCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(StageCheckpoint::bitflag_unused_bits() == 3);
        assert!(StageUnitCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(StageUnitCheckpoint::bitflag_unused_bits() == 0);
        assert!(StoredBlockBodyIndices::bitflag_encoded_bytes() == 1);
        assert!(StoredBlockBodyIndices::bitflag_unused_bits() == 0);
        assert!(StoredBlockOmmers::bitflag_encoded_bytes() == 0);
        assert!(StoredBlockOmmers::bitflag_unused_bits() == 0);
        assert!(StoredBlockWithdrawals::bitflag_encoded_bytes() == 0);
        assert!(StoredBlockWithdrawals::bitflag_unused_bits() == 0);
        assert!(StorageHashingCheckpoint::bitflag_encoded_bytes() == 1);
        assert!(StorageHashingCheckpoint::bitflag_unused_bits() == 6);
        assert!(TxEip1559::bitflag_encoded_bytes() == 4);
        assert!(TxEip1559::bitflag_unused_bits() == 3);
        assert!(TxEip2930::bitflag_encoded_bytes() == 3);
        assert!(TxEip2930::bitflag_unused_bits() == 0);
        assert!(TxEip4844::bitflag_encoded_bytes() == 5);
        assert!(TxEip4844::bitflag_unused_bits() == 6);
        assert!(TxLegacy::bitflag_encoded_bytes() == 3);
        assert!(TxLegacy::bitflag_unused_bits() == 3);
        assert!(Withdrawal::bitflag_encoded_bytes() == 2);
        assert!(Withdrawal::bitflag_unused_bits() == 4);
        assert!(Withdrawals::bitflag_encoded_bytes() == 0);
        assert!(Withdrawals::bitflag_unused_bits() == 0);
    }
}