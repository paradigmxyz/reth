//! Test utilities for `Compact` derive macro

/// Macro to ensure that derived `Compact` types can be extended with new fields while maintaining
/// backwards compatibility.
///
/// Verifies that the unused bits in the bitflag struct remain as expected: `Zero` or `NotZero`. For
/// more on bitflag struct: [`reth_codecs_derive::Compact`].
///
/// Possible failures:
/// ### 1. `NotZero` -> `Zero`
///    This wouldn't allow new fields to be added in the future. Instead, the new field of `T`
/// should be `Option<TExtension>`  to allow for new fields. The new user field should be included
/// in `TExtension` type. **Only then, update the test to expect `Zero` for `T` and
/// add a new test for `TExtension`.**
///
/// **Goal:**
///
///    ```rust,ignore
/// {
///    struct T {
///        // ... other fields
///        ext: Option<TExtension>
///    }
///    
///    // Use an extension type for new fields:
///    struct TExtension {
///        new_field_b: Option<u8>,
///    }
///    
///    // Change tests
///    validate_bitflag_backwards_compat!(T, UnusedBits::Zero);
///    validate_bitflag_backwards_compat!(TExtension, UnusedBits::NotZero);
/// }   
/// ```
/// 
/// ### 2. `Zero` -> `NotZero`
/// If it becomes `NotZero`, it would break backwards compatibility, so there is not an action item,
/// and should be handled with care in a case by case scenario.
#[macro_export]
macro_rules! validate_bitflag_backwards_compat {
    ($type:ty, $expected_unused_bits:expr) => {
        let actual_unused_bits = <$type>::bitflag_unused_bits();

        match $expected_unused_bits {
            UnusedBits::NotZero => {
                assert_ne!(
                    actual_unused_bits,
                    0,
                    "Assertion failed: `bitflag_unused_bits` for type `{}` unexpectedly went from non-zero to zero!",
                    stringify!($type)
                );
            }
            UnusedBits::Zero => {
                assert_eq!(
                    actual_unused_bits,
                    0,
                    "Assertion failed: `bitflag_unused_bits` for type `{}` unexpectedly went from zero to non-zero!",
                    stringify!($type)
                );
            }
        }
    };
}

/// Whether there are zero or more unused bits on `Compact` bitflag struct.
///
/// To be used with [`validate_bitflag_backwards_compat`].
#[derive(Debug)]
pub enum UnusedBits {
    /// Zero bits available for a new field.
    Zero,
    /// Bits available for a new field.
    NotZero,
}

impl UnusedBits {
    /// Returns true if the variant is [`Self::NotZero`].
    pub const fn not_zero(&self) -> bool {
        matches!(self, Self::NotZero)
    }
}
