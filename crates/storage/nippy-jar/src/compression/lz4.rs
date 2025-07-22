use crate::{compression::Compression, NippyJarError};
use serde::{Deserialize, Serialize};

/// Wrapper type for `lz4_flex` that implements [`Compression`].
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct Lz4;

impl Compression for Lz4 {
    fn decompress_to(&self, value: &[u8], dest: &mut Vec<u8>) -> Result<(), NippyJarError> {
        let previous_length = dest.len();

        // Create a mutable slice from the spare capacity
        let spare_capacity = dest.spare_capacity_mut();
        // SAFETY: This is safe because we're using MaybeUninit's as_mut_ptr
        let output = unsafe {
            std::slice::from_raw_parts_mut(
                spare_capacity.as_mut_ptr() as *mut u8,
                spare_capacity.len(),
            )
        };

        match lz4_flex::decompress_into(value, output) {
            Ok(written) => {
                // SAFETY: `compress_into` can only write if there's enough capacity. Therefore, it
                // shouldn't write more than our capacity.
                unsafe {
                    dest.set_len(previous_length + written);
                }
                Ok(())
            }
            Err(_) => Err(NippyJarError::OutputTooSmall),
        }
    }

    fn decompress(&self, value: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        let mut multiplier = 1;

        loop {
            match lz4_flex::decompress(value, multiplier * value.len()) {
                Ok(v) => return Ok(v),
                Err(err) => {
                    multiplier *= 2;
                    if multiplier == 16 {
                        return Err(NippyJarError::Custom(err.to_string()))
                    }
                }
            }
        }
    }

    fn compress_to(&self, src: &[u8], dest: &mut Vec<u8>) -> Result<usize, NippyJarError> {
        let previous_length = dest.len();

        // Create a mutable slice from the spare capacity
        let spare_capacity = dest.spare_capacity_mut();
        // SAFETY: This is safe because we're using MaybeUninit's as_mut_ptr
        let output = unsafe {
            std::slice::from_raw_parts_mut(
                spare_capacity.as_mut_ptr() as *mut u8,
                spare_capacity.len(),
            )
        };

        match lz4_flex::compress_into(src, output) {
            Ok(written) => {
                // SAFETY: `compress_into` can only write if there's enough capacity. Therefore, it
                // shouldn't write more than our capacity.
                unsafe {
                    dest.set_len(previous_length + written);
                }
                Ok(written)
            }
            Err(_) => Err(NippyJarError::OutputTooSmall),
        }
    }

    fn compress(&self, src: &[u8]) -> Result<Vec<u8>, NippyJarError> {
        Ok(lz4_flex::compress(src))
    }
}
