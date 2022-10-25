#![allow(clippy::type_complexity)]
#![doc = include_str!("../README.md")]

pub use crate::{
    codec::*,
    cursor::{Cursor, Iter, IterDup},
    database::Database,
    environment::{
        Environment, EnvironmentBuilder, EnvironmentKind, Geometry, Info, NoWriteMap, Stat,
        WriteMap,
    },
    error::{Error, Result},
    flags::*,
    transaction::{Transaction, TransactionKind, RO, RW},
};

mod codec;
mod cursor;
mod database;
mod environment;
mod error;
mod flags;
mod transaction;

#[cfg(test)]
mod test_utils {
    use super::*;
    use byteorder::{ByteOrder, LittleEndian};
    use tempfile::tempdir;

    type Environment = crate::Environment<NoWriteMap>;

    /// Regression test for https://github.com/danburkert/lmdb-rs/issues/21.
    /// This test reliably segfaults when run against lmbdb compiled with opt level -O3 and newer
    /// GCC compilers.
    #[test]
    fn issue_21_regression() {
        const HEIGHT_KEY: [u8; 1] = [0];

        let dir = tempdir().unwrap();

        let env = {
            let mut builder = Environment::new();
            builder.set_max_dbs(2);
            builder.set_geometry(Geometry {
                size: Some(1_000_000..1_000_000),
                ..Default::default()
            });
            builder.open(dir.path()).expect("open mdbx env")
        };

        for height in 0..1000 {
            let mut value = [0u8; 8];
            LittleEndian::write_u64(&mut value, height);
            let tx = env.begin_rw_txn().expect("begin_rw_txn");
            let index = tx
                .create_db(None, DatabaseFlags::DUP_SORT)
                .expect("open index db");
            tx.put(&index, &HEIGHT_KEY, &value, WriteFlags::empty())
                .expect("tx.put");
            tx.commit().expect("tx.commit");
        }
    }
}
