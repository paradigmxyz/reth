//! Dynamically loading type-safe abstractions

/// Dynamically loaded ExEx entrypoint, that accepts the [`ExExContext`](`reth_exex::ExExContext`)
/// and returns a Future that will be polled by the [`ExExManager`](`reth_exex::ExExManager`).
#[macro_export]
macro_rules! define_exex {
    ($user_fn:ident,<$node:ident>) => {
        #[no_mangle]
        pub extern "C" fn _launch_exex<$node: FullNodeComponents>(
            ctx: $crate::ExExContext<$node>,
        ) -> impl std::future::Future<
            Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>,
        > {
            async move { Ok($user_fn(ctx)) }
        }
    };
}
