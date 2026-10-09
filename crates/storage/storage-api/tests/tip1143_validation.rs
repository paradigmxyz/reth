//! TIP-1143 API acceptance: T-001 through T-004.
//!
//! Proposed public API: ValidatedCode::new(Bytes), original_bytes(), code_hash(),
//! chunks(), descriptor(); CodeChunkDescriptor::new(u32, Vec<B256>), chunk_range(u32).
//! These APIs intentionally do not exist on the pinned pre-implementation base.

use alloy_primitives::{keccak256, Bytes, B256};
use reth_storage_api::{CodeChunkDescriptor, CodeValidationError, ValidatedCode};

#[test]
fn t001_exact_representation_boundaries() {
    for (len, count, last) in [
        (0, 0, 0),
        (24540, 1, 24540),
        (24541, 1, 24541),
        (24542, 2, 1),
        (957100, 40, 1),
        (981640, 40, 24541),
    ] {
        let original = vec![0; len];
        let code = ValidatedCode::new(Bytes::from(original.clone())).unwrap();
        assert_eq!(code.original_bytes().as_ref(), original.as_slice());
        assert_eq!(code.code_hash(), keccak256(&original));
        assert_eq!(code.chunks().len(), count);
        assert_eq!(code.descriptor().is_some(), count > 1);
        for (actual, expected) in code.chunks().iter().zip(original.chunks(24541)) {
            assert_eq!(actual.as_ref(), expected);
        }
        assert_eq!(code.chunks().last().map_or(0, |chunk| chunk.len()), last);
    }
    assert_eq!(
        ValidatedCode::new(Bytes::from(vec![0; 981641])).unwrap_err(),
        CodeValidationError::CodeTooLarge { actual: 981641, maximum: 981640 }
    );
}

#[test]
fn t002_metadata_and_checked_ranges() {
    for count in 2..=40usize {
        for size in [(count - 1) * 24541 + 1, count * 24541] {
            let hashes = vec![B256::repeat_byte(7); count];
            let descriptor = CodeChunkDescriptor::new(size as u32, hashes.clone()).unwrap();
            assert_eq!(descriptor.chunk_range(0), Some(0..24541));
            assert_eq!(descriptor.chunk_range((count - 1) as u32), Some((count - 1) * 24541..size));
            assert_eq!(descriptor.chunk_range(count as u32), None);
            assert_eq!(descriptor.chunk_range(u32::MAX), None);
            for bad_count in [count - 1, count + 1, 41] {
                assert_eq!(
                    CodeChunkDescriptor::new(size as u32, vec![B256::ZERO; bad_count]).unwrap_err(),
                    CodeValidationError::HashCount { expected: count, actual: bad_count }
                );
            }
        }
    }
    for size in [0, 24540, 24541, 981641, u32::MAX] {
        assert_eq!(
            CodeChunkDescriptor::new(size, vec![B256::ZERO; 2]).unwrap_err(),
            CodeValidationError::InvalidCodeSize { size }
        );
    }
}

#[test]
fn t003_all_push_widths_overlap_and_unreachable_code() {
    for width in 1..=32usize {
        for boundary in [24541, 49082] {
            for prefix in [0x00, 0xf3, 0xfe] {
                for available in 0..width {
                    let mut bytes = vec![0; 73623];
                    bytes[0] = prefix;
                    let pc = boundary - available - 1;
                    bytes[pc] = 0x5f + width as u8;
                    bytes[boundary..boundary + width - available].fill(0x5b);
                    let code = ValidatedCode::new(bytes.clone().into()).unwrap();
                    let descriptor = code.descriptor().unwrap();
                    let before = descriptor.preparation((boundary / 24541 - 1) as u32).unwrap();
                    let after = descriptor.preparation((boundary / 24541) as u32).unwrap();
                    assert_eq!(
                        &before.lookahead[..width - available],
                        &bytes[boundary..boundary + width - available]
                    );
                    assert_eq!(after.leading_data_len as usize, width - available);
                    assert_eq!(code.chunks().concat(), bytes);
                }
            }
        }
        for available in 0..width {
            let mut bytes = vec![0; 24542 + available];
            bytes[24541] = 0x5f + width as u8;
            let code = ValidatedCode::new(bytes.into()).unwrap();
            let final_chunk = code.descriptor().unwrap().preparation(1).unwrap();
            assert!(final_chunk.lookahead.is_empty());
            assert_eq!(final_chunk.next_chunk, None);
        }
    }
    // Author-provided STOP boundaries are no longer required.
    for opcode in [0x01, 0x5b, 0x5f, 0xf3, 0xfd, 0xfe, 0xff] {
        let mut bytes = vec![0; 24542];
        bytes[24540] = opcode;
        assert!(ValidatedCode::new(bytes.into()).is_ok());
    }
}

#[test]
fn t004_single_chunk_retains_truncated_push_and_original_bytes() {
    for len in [1, 24540, 24541] {
        for width in 1..=32usize {
            for available in 0..width.min(len) {
                let mut original = vec![0; len];
                original[len - available - 1] = 0x5f + width as u8;
                let code = ValidatedCode::new(original.clone().into()).unwrap();
                assert!(code.descriptor().is_none());
                assert_eq!(code.chunks().len(), 1);
                assert_eq!(code.chunks()[0].as_ref(), original.as_slice());
                assert_eq!(code.code_hash(), keccak256(&original));
            }
        }
        let original = vec![0x01; len];
        let code = ValidatedCode::new(original.clone().into()).unwrap();
        assert_eq!(code.original_bytes().as_ref(), original.as_slice());
        assert!(code.descriptor().is_none());
    }
}
