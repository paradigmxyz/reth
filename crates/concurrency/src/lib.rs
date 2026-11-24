//! Concurrency primitives selected for native execution or fuzz simulation.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

#[cfg(feature = "fuzz")]
use parking_lot as _;
#[cfg(feature = "fuzz")]
use rayon as _;
#[cfg(feature = "fuzz")]
use reth_primitives_traits as _;

pub mod sync;

/// Marks a test that runs in the selected runtime. Requires the `std` feature.
#[cfg(feature = "std")]
pub use reth_concurrency_macros::test;

/// Defines property tests whose cases each run in the selected runtime.
///
/// Takes the same `fn name(pattern in strategy, ...)` items as `proptest::proptest!`, with an
/// optional `#![proptest_config(...)]`. Inputs are generated before each case enters
/// [`sync::Runtime::run`], as it requires, and bodies may use `prop_assert!` and friends. The
/// calling crate must depend on `proptest`.
#[cfg(feature = "std")]
#[macro_export]
macro_rules! proptest {
    (
        $(#![proptest_config($config:expr)])?
        $(
            $(#[$meta:meta])*
            fn $name:ident($($parm:pat in $strategy:expr),+ $(,)?) $body:block
        )*
    ) => {
        ::proptest::proptest! {
            $(#![proptest_config($config)])?
            $(
                $(#[$meta])*
                fn $name($($parm in $strategy),+) {
                    $crate::sync::Runtime::new().run(
                        move || -> ::core::result::Result<(), ::proptest::test_runner::TestCaseError> {
                            let () = $body;
                            ::core::result::Result::Ok(())
                        },
                    )?;
                }
            )*
        }
    };
}
