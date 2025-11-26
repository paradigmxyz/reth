#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}


#[no_mangle]
pub extern "C" fn record_block_inputs(ptr: *const u8, len: usize, out_ptr: *mut u8, out_len: *mut usize) -> i32 {
    if ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr, bytes.len());
        *out_len = bytes.len();
    }
    0
}
#[cfg(test)]
mod tests {
    use super::record_block_inputs;

    #[test]
    fn record_block_inputs_is_deterministic_and_roundtrips() {
        let input = b"arb-stf-inputs-v0:canonical-order-check";
        let mut out = vec![0u8; input.len()];
        let mut out_len: usize = 0;
        let rc1 = unsafe { record_block_inputs(input.as_ptr(), input.len(), out.as_mut_ptr(), &mut out_len as *mut usize) };
        assert_eq!(rc1, 0);
        assert_eq!(out_len, input.len());
        assert_eq!(&out[..out_len], input);

        let mut out2 = vec![0u8; input.len()];
        let mut out_len2: usize = 0;
        let rc2 = unsafe { record_block_inputs(input.as_ptr(), input.len(), out2.as_mut_ptr(), &mut out_len2 as *mut usize) };
        assert_eq!(rc2, 0);
        assert_eq!(out_len2, input.len());
        assert_eq!(&out2[..out_len2], input);

        assert_eq!(&out[..out_len], &out2[..out_len2]);
    }
    #[test]
    fn record_block_inputs_hash_is_deterministic_for_same_inputs() {
        use alloy_primitives::keccak256;
        let input = b"arb-stf-inputs-1234567890";
        let mut out1 = vec![0u8; input.len()];
        let mut out_len1: usize = 0;
        let rc1 = unsafe { record_block_inputs(input.as_ptr(), input.len(), out1.as_mut_ptr(), &mut out_len1 as *mut usize) };
        assert_eq!(rc1, 0);

        let mut out2 = vec![0u8; input.len()];
        let mut out_len2: usize = 0;
        let rc2 = unsafe { record_block_inputs(input.as_ptr(), input.len(), out2.as_mut_ptr(), &mut out_len2 as *mut usize) };
        assert_eq!(rc2, 0);

        assert_eq!(&out1[..out_len1], &out2[..out_len2]);
        let h1 = keccak256(&out1[..out_len1]);
        let h2 = keccak256(&out2[..out_len2]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn record_block_inputs_hash_differs_for_different_inputs() {
        use alloy_primitives::keccak256;
        let a = b"aaaa";
        let b = b"bbbb";
        let mut out_a = vec![0u8; a.len()];
        let mut out_b = vec![0u8; b.len()];
        let mut len_a: usize = 0;
        let mut len_b: usize = 0;

        unsafe {
            record_block_inputs(a.as_ptr(), a.len(), out_a.as_mut_ptr(), &mut len_a as *mut usize);
            record_block_inputs(b.as_ptr(), b.len(), out_b.as_mut_ptr(), &mut len_b as *mut usize);
        }

        let ha = keccak256(&out_a[..len_a]);
        let hb = keccak256(&out_b[..len_b]);
        assert_ne!(ha, hb);
    }

}
