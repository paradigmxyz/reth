#![allow(missing_docs, non_snake_case, unreachable_pub)]

use paste::paste;
use reth_db::table::{Compress, Decode, Decompress, Encode, Table};

mod utils;
use utils::*;

macro_rules! impl_iai_inner {
    (
        $(($name:ident, $mod:ident, $compress:ident, $decompress:ident, $encode:ident, $decode:ident, $seqread:ident, $randread:ident, $seqwrite:ident, $randwrite:ident))+
    ) => {
        $(
            mod $mod {
                use super::*;
                use std::hint::black_box;

                pub fn $compress() {
                    for (_, _, v, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        black_box(v.compress());
                    }
                }

                pub fn $decompress() {
                    for (_, _, _, comp) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        let _ = black_box(<reth_db::tables::$name as Table>::Value::decompress(comp));
                    }
                }

                pub fn $encode() {
                    for (k, _, _, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        black_box(k.encode());
                    }
                }

                pub fn $decode() {
                    for (_, enc, _, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        let _ = black_box(<reth_db::tables::$name as Table>::Key::decode(enc));
                    }
                }

                #[allow(dead_code)]
                pub fn $seqread() {}

                #[allow(dead_code)]
                pub fn $randread() {}

                #[allow(dead_code)]
                pub fn $seqwrite() {}

                #[allow(dead_code)]
                pub fn $randwrite() {}
            }
            use $mod::*;
        )+

        iai::main!(
            $(
                $compress,
                $decompress,
                $encode,
                $decode,
            )+
        );
    };
}

macro_rules! impl_iai {
    ($($name:ident),+) => {
            paste! {
                impl_iai_inner!(
                    $(
                        ( $name, [<$name _mod>], [<$name _ValueCompress>], [<$name _ValueDecompress>], [<$name _ValueEncode>], [<$name _ValueDecode>], [<$name _SeqRead>], [<$name _RandomRead>], [<$name _SeqWrite>], [<$name _RandomWrite>])
                    )+
                );
            }
    };
}

impl_iai!(
    CanonicalHeaders,
    HeaderTD,
    HeaderNumbers,
    Headers,
    BlockBodyIndices,
    BlockOmmers,
    TxHashNumber,
    Transactions,
    PlainStorageState,
    PlainAccountState
);
