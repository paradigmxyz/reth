#[macro_export]
macro_rules! test_bitflag_unused_bits {
    ($type:ty, $expected_unused_bits:expr) => {
        if $expected_unused_bits.not_zero() {
            assert_ne!(
                <$type>::bitflag_unused_bits(),
                0,
                "Assertion failed: `bitflag_unused_bits` for type `{}` is zero!",
                stringify!($type)
            );
        }
    };
}

/// Whether there are zero or more unused bits on `Compact` bitflag struct.
///
/// To be used with [`test_bitflag_unused_bits`].
#[derive(Debug)]
pub enum UnusedBits {
    Zero,
    NotZero,
}

impl UnusedBits {
    pub fn not_zero(&self) -> bool {
        matches!(self, Self::NotZero)
    }
}
