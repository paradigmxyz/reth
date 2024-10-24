//! Type-safe abstractions for Dynamically Loaded ExExes

/// ExEx launch function dynamic library symbol name.
pub(crate) const LAUNCH_EXEX_FN: &[u8] = b"_launch_exex";

/// Dynamically loads an ExEx entrypoint, which accepts a user-defined function representing the
/// core ExEx logic. The provided function must take an [`ExExContextDyn`](`crate::ExExContextDyn`)
/// as its argument.
///
/// # Returns
/// A Future that will be polled by the [`ExExManager`](`crate::ExExManager`).
///
/// ## Example usage:
/// ```rust
/// use reth_exex::{define_exex, ExExContextDyn};
/// use reth_node_api::FullNodeComponents;
/// use std::future::Future;
///
/// // Create a function to produce ExEx logic
/// async fn exex(
///     _ctx: ExExContextDyn,
/// ) -> eyre::Result<impl std::future::Future<Output = eyre::Result<()>>> {
///     let _exex = async move { Ok(()) };
///     Ok(_exex)
/// }
///
/// // Use the macro to generate the entrypoint function
/// define_exex!(exex);
/// ```
#[macro_export]
macro_rules! define_exex {
    ($user_fn:ident) => {
        #[no_mangle]
        pub extern "Rust" fn _launch_exex(
            ctx: $crate::ExExContextDyn,
        ) -> impl std::future::Future<
            Output = eyre::Result<impl std::future::Future<Output = eyre::Result<()>> + Send>,
        > {
            $user_fn(ctx)
        }
    };
}
