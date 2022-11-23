use crate::db::{
    models::{accounts::AccountBeforeTx, StoredBlockBody},
    Compress, Decompress, Error,
};
use reth_codecs::Compact;
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
                fn decompress<B: Into<bytes::Bytes>>(value: B) -> Result<$name, Error> {
                    let value = value.into();
                    let (obj, _) = Compact::from_compact(&value, value.len());
                    Ok(obj)
                }
            }
        )+
    };
}

impl_compression_for_compact!(Header, Account, Log, Receipt, TxType, StorageEntry, StoredBlockBody);
impl_compression_for_compact!(AccountBeforeTx);
