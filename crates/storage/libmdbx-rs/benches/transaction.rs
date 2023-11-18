mod utils;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ffi::*;
use libc::size_t;
use rand::{prelude::SliceRandom, SeedableRng};
use rand_xorshift::XorShiftRng;
use reth_libmdbx::{ObjectLength, WriteFlags};
use std::ptr;
use utils::*;

fn bench_get_rand(c: &mut Criterion) {
    let n = 100u32;
    let (_dir, env) = setup_bench_db(n);
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();

    let mut keys: Vec<String> = (0..n).map(get_key).collect();
    keys.shuffle(&mut XorShiftRng::from_seed(Default::default()));

    c.bench_function("bench_get_rand", |b| {
        b.iter(|| {
            let mut i = 0usize;
            for key in &keys {
                i += *txn.get::<ObjectLength>(db.dbi(), key.as_bytes()).unwrap().unwrap();
            }
            black_box(i);
        })
    });
}

fn bench_get_rand_raw(c: &mut Criterion) {
    let n = 100u32;
    let (_dir, env) = setup_bench_db(n);
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();

    let mut keys: Vec<String> = (0..n).map(get_key).collect();
    keys.shuffle(&mut XorShiftRng::from_seed(Default::default()));

    let dbi = db.dbi();

    let mut key_val: MDBX_val = MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };
    let mut data_val: MDBX_val = MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

    c.bench_function("bench_get_rand_raw", |b| {
        b.iter(|| unsafe {
            txn.with_raw_tx_ptr(|txn| {
                let mut i: size_t = 0;
                for key in &keys {
                    key_val.iov_len = key.len() as size_t;
                    key_val.iov_base = key.as_bytes().as_ptr() as *mut _;

                    mdbx_get(txn, dbi, &key_val, &mut data_val);

                    i += key_val.iov_len;
                }
                black_box(i);
            });
        })
    });
}

fn bench_put_rand(c: &mut Criterion) {
    let n = 100u32;
    let (_dir, env) = setup_bench_db(0);

    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.prime_for_permaopen(db);
    let db = txn.commit_and_rebind_open_dbs().unwrap().1.remove(0);

    let mut items: Vec<(String, String)> = (0..n).map(|n| (get_key(n), get_data(n))).collect();
    items.shuffle(&mut XorShiftRng::from_seed(Default::default()));

    c.bench_function("bench_put_rand", |b| {
        b.iter(|| {
            let txn = env.begin_rw_txn().unwrap();
            for (key, data) in items.iter() {
                txn.put(db.dbi(), key, data, WriteFlags::empty()).unwrap();
            }
        })
    });
}

fn bench_put_rand_raw(c: &mut Criterion) {
    let n = 100u32;
    let (_dir, env) = setup_bench_db(0);

    let mut items: Vec<(String, String)> = (0..n).map(|n| (get_key(n), get_data(n))).collect();
    items.shuffle(&mut XorShiftRng::from_seed(Default::default()));

    let dbi = env.begin_ro_txn().unwrap().open_db(None).unwrap().dbi();

    let mut key_val: MDBX_val = MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };
    let mut data_val: MDBX_val = MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

    c.bench_function("bench_put_rand_raw", |b| {
        b.iter(|| unsafe {
            let mut txn: *mut MDBX_txn = ptr::null_mut();
            env.with_raw_env_ptr(|env| {
                mdbx_txn_begin_ex(env, ptr::null_mut(), 0, &mut txn, ptr::null_mut());

                let mut i: ::libc::c_int = 0;
                for (key, data) in items.iter() {
                    key_val.iov_len = key.len() as size_t;
                    key_val.iov_base = key.as_bytes().as_ptr() as *mut _;
                    data_val.iov_len = data.len() as size_t;
                    data_val.iov_base = data.as_bytes().as_ptr() as *mut _;

                    i += mdbx_put(txn, dbi, &key_val, &mut data_val, 0);
                }
                assert_eq!(0, i);
                mdbx_txn_abort(txn);
            });
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(pprof::criterion::PProfProfiler::new(100, pprof::criterion::Output::Flamegraph(None)));
    targets = bench_get_rand, bench_get_rand_raw, bench_put_rand, bench_put_rand_raw
}
criterion_main!(benches);
