#![allow(non_snake_case)]

use iai::main;

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

include!("./utils.rs");
