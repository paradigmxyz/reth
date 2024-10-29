#![allow(missing_docs, non_snake_case, unreachable_pub)]

use iai_callgrind::{
    library_benchmark, library_benchmark_group, LibraryBenchmarkConfig, RegressionConfig,
};
use paste::paste;
use reth_db_api::table::{Compress, Decode, Decompress, Encode, Table};

mod utils;
use utils::*;

macro_rules! impl_iai_callgrind_inner {
    (
        $(($name:ident, $group_name:ident, $mod:ident, $compress:ident, $decompress:ident, $encode:ident, $decode:ident, $seqread:ident, $randread:ident, $seqwrite:ident, $randwrite:ident))+
    ) => {
        use std::hint::black_box;
        $(
                #[library_benchmark]
                pub fn $compress() {
                    for (_, _, v, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        black_box(v.compress());
                    }
                }

                #[library_benchmark]
                pub fn $decompress() {
                    for (_, _, _, comp) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        let _ = black_box(<reth_db::tables::$name as Table>::Value::decompress(&comp));
                    }
                }

                #[library_benchmark]
                pub fn $encode() {
                    for (k, _, _, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        black_box(k.encode());
                    }
                }

                #[library_benchmark]
                pub fn $decode() {
                    for (_, enc, _, _) in black_box(load_vectors::<reth_db::tables::$name>()) {
                        let _ = black_box(<reth_db::tables::$name as Table>::Key::decode(&enc));
                    }
                }

                #[allow(dead_code)]
                pub const fn $seqread() {}

                #[allow(dead_code)]
                pub const fn $randread() {}

                #[allow(dead_code)]
                pub const fn $seqwrite() {}

                #[allow(dead_code)]
                pub const fn $randwrite() {}


                library_benchmark_group!(
                    name = $group_name;
                    config = LibraryBenchmarkConfig::default()
                        .regression(
                            RegressionConfig::default().fail_fast(false)
                        );
                    benchmarks =
                        $compress,
                        $decompress,
                        $encode,
                        $decode,
                );
        )+

        iai_callgrind::main!(
                config = LibraryBenchmarkConfig::default();
                library_benchmark_groups = $($group_name),+);
    };
}

macro_rules! impl_iai_callgrind {
    ($($name:ident),+) => {
            paste! {
                impl_iai_callgrind_inner!(
                    $(
                        ( $name, [<$name _group>],[<$name _mod>], [<$name _ValueCompress>], [<$name _ValueDecompress>], [<$name _ValueEncode>], [<$name _ValueDecode>], [<$name _SeqRead>], [<$name _RandomRead>], [<$name _SeqWrite>], [<$name _RandomWrite>])
                    )+
                );
            }
    };
}

impl_iai_callgrind!(
    CanonicalHeaders,
    HeaderTerminalDifficulties,
    HeaderNumbers,
    Headers,
    BlockBodyIndices,
    BlockOmmers,
    TransactionHashNumbers,
    Transactions,
    PlainStorageState,
    PlainAccountState
);
