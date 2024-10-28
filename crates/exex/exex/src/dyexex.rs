//! Type-safe abstractions for Dynamically Loaded ExExes

use std::{
    env::consts::{DLL_PREFIX, DLL_SUFFIX},
    fmt::Debug,
    future::Future,
    path::Path,
    pin::Pin,
    sync::Arc,
};

use eyre::{OptionExt, Result};
use futures::future::BoxFuture;
use libloading::{Library, Symbol};

use crate::ExExContextDyn;

/// ExEx launch function dynamic library symbol name.
const LAUNCH_EXEX_FN: &[u8] = b"_launch_exex";

/// Regarding ExEx launch entrypoint future
type ExExFut = BoxFuture<'static, BoxFuture<'static, Result<()>>>;

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
        ) -> futures::future::BoxFuture<
            'static,
            futures::future::BoxFuture<'static, eyre::Result<()>>,
        > {
            futures::FutureExt::boxed(async move {
                Box::pin(async move { exex(ctx).await })
                    as futures::future::BoxFuture<'static, eyre::Result<()>>
            })
        }
    };
}

#[derive(Debug, Default)]
/// Dynamic ExEx loader
pub struct DyExExLoader {
    /// List of loaded Dynamically Loaded ExExes
    pub loaded: Vec<LoadedExEx>,
}

impl DyExExLoader {
    /// Initializes a Dynamic ExEx loader.
    /// TODO(0xurb) - path to datadir here and loaded list with capacity of dir inner files.
    pub const fn new() -> Self {
        Self { loaded: Vec::new() }
    }

    /// Loads a dynamically loaded ExEx from a given path.
    ///
    /// # Safety
    ///
    /// The dynamically loaded ExEx library **must** contain a function named `_launch_exex`
    /// with the correct type signature, that resolves to exact function pointer.
    ///
    /// Otherwise, behavior is undefined.
    /// See also [`Library::get`] for more information on what
    /// restrictions apply to `_launch_exex` symbol.
    #[allow(clippy::type_complexity)]
    pub unsafe fn load(&mut self, path: impl AsRef<Path>, ctx: ExExContextDyn) -> Result<()> {
        let path = path.as_ref();

        // Dynamically loaded ExEx id it's a filename with stripped [`DLL_PREFIX`] and
        // [`DLL_SUFFIX`]
        let lib_name = path
            .file_name()
            .ok_or_eyre("cannot obtain a filename for dyexex")?
            .to_str()
            .ok_or_eyre("dyexex name is not a valid UTF-8 encoded string")?
            .strip_prefix(DLL_PREFIX)
            .ok_or_else(|| {
                eyre::eyre!(
                    "dyexex is not a shared dynamic library (has no prefix: `{DLL_PREFIX}`)"
                )
            })?
            .strip_suffix(DLL_SUFFIX)
            .ok_or_else(|| {
                eyre::eyre!(
                    "dyexex is not a shared dynamic library (has no suffix: `{DLL_SUFFIX}`)"
                )
            })?;

        let lib = Library::new(path)?;
        let symbol: Symbol<
            '_,
            unsafe fn(
                ExExContextDyn,
            )
                -> *mut (dyn Future<Output = BoxFuture<'static, Result<()>>> + Send),
        > = lib.get(LAUNCH_EXEX_FN)?;

        let raw_func_pointer = symbol(ctx);
        if raw_func_pointer.is_null() {
            return Err(eyre::eyre!("Failed to load future from dynamic library"));
        }

        // SAFETY: We guarantee that the pointed data is pinned, loaded DLL symbol
        // function pointer resolves to the pinned future.
        let exex_fut = Pin::new_unchecked(Box::from_raw(raw_func_pointer));

        self.loaded.push(LoadedExEx::new(lib_name, lib, exex_fut));

        Ok(())
    }
}

/// Dynamically Loaded ExEx representation
pub struct LoadedExEx {
    /// ExEx id
    id: String,
    /// Loaded [`Library`] pointer
    lib: Arc<Library>,
    /// Regarding ExEx launch entrypoint future
    exex_fut: ExExFut,
}

impl Debug for LoadedExEx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedExEx")
            .field("id", &self.id)
            .field("lib", &self.lib)
            .field("exex_fut", &"...")
            .finish()
    }
}

impl LoadedExEx {
    /// Create a new [`LoadedExEx`].
    fn new(lib_name: impl AsRef<str>, lib: Library, exex_fut: ExExFut) -> Self {
        Self { id: lib_name.as_ref().to_owned(), lib: Arc::new(lib), exex_fut }
    }

    /// Returns this ExEx id and launch entrypoint future.
    #[inline(always)]
    pub fn into_id_and_launch_fn(self) -> (String, ExExFut) {
        (self.id, self.exex_fut)
    }
}
