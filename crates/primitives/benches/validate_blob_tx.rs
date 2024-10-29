#![allow(missing_docs)]

use alloy_consensus::TxEip4844;
use alloy_eips::eip4844::{env_settings::EnvKzgSettings, MAX_BLOBS_PER_BLOCK};
use alloy_primitives::hex;
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner::{RngAlgorithm, TestRng, TestRunner},
};
use proptest_arbitrary_interop::arb;
use reth_primitives::BlobTransactionSidecar;

// constant seed to use for the rng
const SEED: [u8; 32] = hex!("1337133713371337133713371337133713371337133713371337133713371337");

/// Benchmarks EIP-48444 blob validation.
fn blob_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Blob Transaction KZG validation");

    for num_blobs in 1..=MAX_BLOBS_PER_BLOCK {
        println!("Benchmarking validation for tx with {num_blobs} blobs");
        validate_blob_tx(&mut group, "ValidateBlob", num_blobs as u64, EnvKzgSettings::Default);
    }
}

fn validate_blob_tx(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    num_blobs: u64,
    kzg_settings: EnvKzgSettings,
) {
    let setup = || {
        let config = ProptestConfig::default();
        let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &SEED);
        let mut runner = TestRunner::new_with_rng(config, rng);

        // generate tx and sidecar
        let mut tx = arb::<TxEip4844>().new_tree(&mut runner).unwrap().current();
        let mut blob_sidecar =
            arb::<BlobTransactionSidecar>().new_tree(&mut runner).unwrap().current();

        while blob_sidecar.blobs.len() < num_blobs as usize {
            let blob_sidecar_ext =
                arb::<BlobTransactionSidecar>().new_tree(&mut runner).unwrap().current();

            // extend the sidecar with the new blobs
            blob_sidecar.blobs.extend(blob_sidecar_ext.blobs);
            blob_sidecar.proofs.extend(blob_sidecar_ext.proofs);
            blob_sidecar.commitments.extend(blob_sidecar_ext.commitments);

            if blob_sidecar.blobs.len() > num_blobs as usize {
                blob_sidecar.blobs.truncate(num_blobs as usize);
                blob_sidecar.proofs.truncate(num_blobs as usize);
                blob_sidecar.commitments.truncate(num_blobs as usize);
            }
        }

        tx.blob_versioned_hashes = blob_sidecar.versioned_hashes().collect();

        (tx, blob_sidecar)
    };

    let group_id = format!("validate_blob | num blobs: {num_blobs} | {description}");

    // for now we just use the default SubPoolLimit
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |(tx, blob_sidecar)| {
            if let Err(err) =
                std::hint::black_box(tx.validate_blob(&blob_sidecar, kzg_settings.get()))
            {
                println!("Validation failed: {err:?}");
            }
        });
    });
}

criterion_group!(validate_blob, blob_validation);
criterion_main!(validate_blob);
