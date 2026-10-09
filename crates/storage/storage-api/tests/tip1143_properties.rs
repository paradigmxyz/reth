//! T-005: deterministic construction properties with an independent slice/hash oracle.
//! No optional draft Tempo extension codec is requested by this acceptance API.

use alloy_primitives::keccak256;
use reth_storage_api::ValidatedCode;

#[test]
fn t005_fixed_seed_original_payload_properties() {
    // Xorshift64* seed is fixed; generation never consults the validator.
    let mut seed = 0x1143_2457_6040_cafe_u64;
    let mut sizes = vec![0, 24540, 24541, 24542, 957100, 981640];
    for _ in 0..128 {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        sizes.push((seed.wrapping_mul(2685821657736338717) % 981641) as usize);
    }
    assert_eq!(sizes.len(), 134);
    for size in sizes {
        let mut original = vec![0; size];
        for (index, chunk) in original.chunks_mut(24541).enumerate() {
            // Complete PUSH32s with hostile-looking immediate bytes, and STOP padding.
            for instruction in chunk.chunks_exact_mut(33) {
                instruction[0] = 0x7f;
                instruction[1..].fill((index as u8).wrapping_mul(17).wrapping_add(0x60));
            }
            // 24541 % 33 == 24, so this is always a decoded opcode.
            if chunk.len() == 24541 {
                chunk[24540] = 0;
            }
        }
        let code = ValidatedCode::new(original.clone().into()).unwrap();
        assert_eq!(code.code_hash(), keccak256(&original));
        assert_eq!(code.original_bytes().as_ref(), original.as_slice());
        assert_eq!(code.chunks().len(), size.div_ceil(24541));
        assert_eq!(code.chunks().concat(), original);
        for (index, payload) in code.chunks().iter().enumerate() {
            let expected = &original[index * 24541..((index + 1) * 24541).min(size)];
            assert_eq!(payload.as_ref(), expected);
            if let Some(descriptor) = code.descriptor() {
                assert_eq!(descriptor.chunk_hashes()[index], keccak256(expected));
            }
        }
    }
}
