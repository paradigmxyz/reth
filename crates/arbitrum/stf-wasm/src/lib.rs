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
