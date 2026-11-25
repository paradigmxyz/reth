//! Procedural macros for `reth-concurrency`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, spanned::Spanned, Error, ItemFn};

/// Marks a test that runs inside `reth_concurrency::sync::Runtime::run`.
///
/// Expands to a `#[test]` whose body moves into a `Runtime::run` closure, so the test runs
/// inline by default and under simulation with the `fuzz` feature. Other attributes, such as
/// `#[should_panic]`, are kept. Inside `proptest!`, it replaces `#[test]` and runs every case,
/// including input generation, in one runtime.
#[proc_macro_attribute]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        let args = proc_macro2::TokenStream::from(args);
        return Error::new(args.span(), "`reth_concurrency::test` takes no arguments")
            .to_compile_error()
            .into();
    }

    let ItemFn { attrs, vis, sig, block } = parse_macro_input!(item as ItemFn);
    if let Some(asyncness) = sig.asyncness {
        return Error::new(
            asyncness.span(),
            "`reth_concurrency::test` does not support async tests",
        )
        .to_compile_error()
        .into();
    }
    if !sig.inputs.is_empty() {
        return Error::new(
            sig.inputs.span(),
            "`reth_concurrency::test` functions take no arguments",
        )
        .to_compile_error()
        .into();
    }

    quote! {
        #[::core::prelude::v1::test]
        #(#attrs)*
        #vis #sig {
            ::reth_concurrency::sync::Runtime::new().run(move || #block)
        }
    }
    .into()
}
