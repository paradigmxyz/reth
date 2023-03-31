use crate::{
    table::{Compress, Decompress},
    tables::models::*,
    Error,
};
use reth_codecs::{main_codec, Compact};
use reth_primitives::*;

/// Implements compression for Compact type.
macro_rules! impl_compression_for_compact {
    ($($name:tt),+) => {
        $(
            impl Compress for $name
            {
                type Compressed = Vec<u8>;

                fn compress(self) -> Self::Compressed {
                    let mut buf = vec![];
                    let _  = Compact::to_compact(self, &mut buf);
                    buf
                }
            }

            impl Decompress for $name
            {
                fn decompress<B: AsRef<[u8]>>(value: B) -> Result<$name, Error> {
                    let value = value.as_ref();
                    let (obj, _) = Compact::from_compact(&value, value.len());
                    Ok(obj)
                }
            }
        )+
    };
}

impl_compression_for_compact!(
    Header,
    Account,
    Log,
    Receipt,
    TxType,
    StorageEntry,
    StorageTrieEntry,
    StoredBlockBody,
    StoredBlockOmmers,
    StoredBlockWithdrawals,
    Bytecode,
    ProofCheckpoint
);
impl_compression_for_compact!(AccountBeforeTx, TransactionSigned);
impl_compression_for_compact!(CompactU256);
impl_compression_for_compact!(H256, H160);

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
