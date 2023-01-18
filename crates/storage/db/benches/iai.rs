#![allow(non_snake_case)]

use iai::main;
use reth_db::table::*;

macro_rules! impl_iai {
    ($($name:tt),+) => {
        $(
            mod $name {
                use crate::load_vectors;
                use reth_db::{table::*};
                use iai::{black_box};

                pub fn encode() {
                    let pair = load_vectors::<reth_db::tables::$name>();

                    black_box(
                        for (k, enc, v, comp) in pair.into_iter() {
                            k.encode();
                            let _ = <reth_db::tables::$name as Table>::Key::decode(enc);
                            v.compress();
                            let _ = <reth_db::tables::$name as Table>::Value::decompress(comp);
                        }
                    );
                }
            }
        )+

        $(
            use $name::{encode as $name};
        )+

        main!(
            $(
                $name,
            )+
        );
    };
}

impl_iai!(
    CanonicalHeaders,
    HeaderTD,
    HeaderNumbers,
    Headers,
    BlockOmmers,
    NonCanonicalTransactions,
    Transactions,
    TxHashNumber,
    Receipts,
    Logs,
    PlainAccountState,
    PlainStorageState,
    Bytecodes,
    AccountHistory,
    StorageHistory,
    AccountChangeSet,
    StorageChangeSet,
    TxSenders,
    Config,
    SyncStage
);

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
fn load_vectors<T>() -> Vec<(T::Key, bytes::Bytes, T::Value, bytes::Bytes)>
where
    T: Table + Default,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    let encoded: bytes::Bytes = bytes::Bytes::copy_from_slice(T::Key::default().encode().as_ref());
    let compressed: bytes::Bytes =
        bytes::Bytes::copy_from_slice(T::Value::default().compress().as_ref());

    let row = (T::Key::default(), encoded, T::Value::default(), compressed);
    vec![row.clone(), row.clone(), row]
}
