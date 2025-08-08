#![cfg_attr(not(feature = "std"), no_std)]

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
}
