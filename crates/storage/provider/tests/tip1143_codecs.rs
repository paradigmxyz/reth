//! T-032: independent descriptor wire vectors and raw MDBX corruption.
//!
//! Versioned descriptor framing includes original size, ordered hashes, and bounded
//! per-context lookahead/leading data. This is a database format, independently
//! selected from account extension framing.

use alloy_primitives::{keccak256, B256};
use reth_db_api::{
    table::{Compress, Decompress},
    tables::{self, RawTable, RawValue},
    transaction::DbTxMut,
};
use reth_provider::{test_utils::create_test_provider_factory, StateProviderFactory};
use reth_storage_api::{CodeChunkDescriptor, CodeChunkReader, DBProvider};

#[test]
fn t032_descriptor_vectors_and_raw_malformed_records() {
    for count in 2..=40usize {
        for size in [(count - 1) * 24541 + 1, count * 24541] {
            let hashes = (0..count).map(|i| B256::repeat_byte(i as u8)).collect::<Vec<_>>();
            let descriptor = CodeChunkDescriptor::new(size as u32, hashes.clone()).unwrap();
            let mut expected = vec![1];
            expected.extend_from_slice(&(size as u32).to_be_bytes());
            for (index, hash) in hashes.iter().enumerate() {
                expected.extend_from_slice(hash.as_slice());
                expected.extend_from_slice(&[
                    0,
                    0,
                    (size - ((index + 1) * 24541).min(size)).min(32) as u8,
                    if index + 1 < count { (index + 1) as u8 } else { 0xff },
                ]);
                expected.extend(core::iter::repeat_n(
                    0,
                    (size - ((index + 1) * 24541).min(size)).min(32),
                ));
            }
            assert_eq!(descriptor.clone().compress().as_ref(), expected.as_slice());
            let decoded = CodeChunkDescriptor::decompress(&expected).unwrap();
            assert_eq!(decoded.code_size(), size as u32);
            assert_eq!(decoded.chunk_hashes(), hashes);
            let mut malformed = Vec::new();
            for length in [0, 1, 2, 3, 4, expected.len() - 1] {
                malformed.push(expected[..length].to_vec());
            }
            let mut trailing = expected.clone();
            trailing.push(0);
            malformed.push(trailing);
            for invalid_size in [0u32, 24541, 981641, u32::MAX] {
                let mut bytes = expected.clone();
                bytes[1..5].copy_from_slice(&invalid_size.to_be_bytes());
                malformed.push(bytes);
            }
            for bytes in malformed {
                assert!(CodeChunkDescriptor::decompress(&bytes).is_err());
                let factory = create_test_provider_factory();
                let hash = keccak256(b"malformed persisted descriptor");
                let writer = factory.provider_rw().unwrap();
                writer
                    .tx_ref()
                    .put::<RawTable<tables::BytecodeChunkDescriptors>>(
                        hash.into(),
                        RawValue::from_vec(bytes),
                    )
                    .unwrap();
                writer.commit().unwrap();
                assert!(factory.latest().unwrap().get_code_chunk_by_hash(&hash, 0).is_err());
            }
        }
    }
}

/// T-032: bounded deterministic fuzz replay checks decoding independently of encoding.
/// Seed 0x1143c0decafe1234, 512 samples, maximum input length 1349 bytes.
#[test]
fn t032_bounded_descriptor_parser_fuzz_replay() {
    let mut seed = 0x1143_c0de_cafe_1234_u64;
    for sample in 0..512usize {
        let length = sample * 37 % 1350;
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            bytes.push(seed as u8);
        }
        // Half the corpus uses a plausible header so count/framing validation is reached.
        if sample % 2 == 0 && length >= 4 {
            let count = 2 + sample % 39;
            bytes[..4].copy_from_slice(&((count * 24541) as u32).to_be_bytes());
        }
        // The random corpus is not a valid version-1 descriptor. Ensure rejection is panic-free.
        assert!(
            CodeChunkDescriptor::decompress(&bytes).is_err(),
            "sample {sample}, length {length}"
        );
    }
}
