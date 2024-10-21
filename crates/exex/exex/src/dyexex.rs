//! Type-safe abstractions for Dynamically Loaded ExExes

/// Dynamically loads an ExEx entrypoint, which accepts a user-defined function representing the
/// core ExEx logic. The provided function must take an [`ExExContext`](`crate::ExExContext`) as its
/// argument.
///
/// # Returns
/// A Future that will be polled by the [`ExExManager`](`crate::ExExManager`).
///
/// ## Example usage:
/// ```rust
/// use reth_exex::{define_exex, ExExContext};
/// use reth_node_api::FullNodeComponents;
/// use std::future::Future;
///
/// // Create a function to produce ExEx logic
/// async fn exex<Node: FullNodeComponents>(
///     _ctx: ExExContext<Node>,
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
        pub extern "C" fn _launch_exex<Node: FullNodeComponents>(
            ctx: $crate::ExExContext<Node>,
        ) -> impl std::future::Future<
            Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>,
        > {
            $user_fn(ctx)
        }
    };
}
