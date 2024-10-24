//! Type-safe abstractions for Dynamically Loaded ExExes

use std::{future::Future, path::Path};

use eyre::Result;
use libloading::{Library, Symbol};

use crate::ExExContextDyn;

/// ExEx launch function dynamic library symbol name.
const LAUNCH_EXEX_FN: &[u8] = b"_launch_exex";

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
/// async fn exex(_ctx: ExExContextDyn) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
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
    (async move |ctx| {
        Ok($user_fn:ident(ctx))
    }) => {
        #[allow(no_mangle_generic_items, unreachable_pub)]
        #[no_mangle]
        pub async extern "Rust" fn _launch_exex(
            ctx: $crate::ExExContextDyn,
        ) -> eyre::Result<impl std::future::Future<Output = eyre::Result<()>> + Send> {
            Ok($user_fn(ctx))
        }
    };
}

/// Loads a dynamically loaded ExEx from a given path.
#[allow(clippy::type_complexity)]
pub fn load(
    path: impl AsRef<Path>,
    ctx: ExExContextDyn,
) -> Result<Box<dyn Future<Output = dyn Future<Output = Result<()>> + Send> + Send>> {
    let lib = unsafe { Library::new(path.as_ref()) }?;
    let raw_func_pointer: Symbol<
        '_,
        unsafe fn(
            ExExContextDyn,
        )
            -> *mut (dyn Future<Output = dyn Future<Output = eyre::Result<()>> + Send>
             + Send),
    > = unsafe { lib.get(LAUNCH_EXEX_FN)? };

    let exex = unsafe { Box::from_raw(raw_func_pointer(ctx)) };
    Ok(exex)
}
