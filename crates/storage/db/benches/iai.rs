#![allow(dead_code, unused_imports, non_snake_case)]

use iai::main;
use paste::paste;

macro_rules! impl_iai_inner {
    (
        $(($name:tt, $mod:tt, $compress:tt, $decompress:tt, $encode:tt, $decode:tt, $seqread:tt, $randread:tt, $seqwrite:tt, $randwrite:tt))+
    ) => {
            $(
                mod $mod {
                    use iai::{black_box};
                    include!("./utils.rs");

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
                    pub fn $seqread() {}
                    pub fn $randread() {}
                    pub fn $seqwrite() {}
                    pub fn $randwrite() {}
                }
                use $mod::*;
            )+

        main!(
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
    ($($name:tt),+) => {
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
