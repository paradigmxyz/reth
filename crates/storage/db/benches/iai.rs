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
                        let pair = load_vectors::<reth_db::tables::$name>();

                        black_box(
                            for (_, _, v, _) in pair {
                                v.compress();
                            }
                        );
                    }
                    pub fn $decompress() {
                        let pair = load_vectors::<reth_db::tables::$name>();

                        black_box(
                            for (_, _, _, comp) in pair {
                                let _ = <reth_db::tables::$name as Table>::Value::decompress(comp);
                            }
                        );
                    }
                    pub fn $encode() {
                        let pair = load_vectors::<reth_db::tables::$name>();

                        black_box(
                            for (k, _, _, _) in pair {
                                k.encode();
                            }
                        );
                    }
                    pub fn $decode() {
                        let pair = load_vectors::<reth_db::tables::$name>();

                        black_box(
                            for (_, enc, _, _) in pair {
                                let _ = <reth_db::tables::$name as Table>::Key::decode(enc);
                            }
                        );
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
    BlockBodies,
    BlockOmmers,
    TxHashNumber,
    BlockTransitionIndex,
    TxTransitionIndex,
    SyncStage,
    Transactions,
    PlainStorageState,
    PlainAccountState
);
