//! Derive macros for the Compact codec traits.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(unreachable_pub, missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use proc_macro::{TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput};

mod arbitrary;
mod compact;

#[proc_macro_derive(Compact, attributes(maybe_zero))]
pub fn derive(input: TokenStream) -> TokenStream {
    let is_zstd = false;
    compact::derive(input, is_zstd)
}

#[proc_macro_derive(CompactZstd, attributes(maybe_zero))]
pub fn derive_zstd(input: TokenStream) -> TokenStream {
    let is_zstd = true;
    compact::derive(input, is_zstd)
}

/// This code implements the main codec. If the codec supports it, it will also provide the [`derive_arbitrary()`] function, which automatically implements arbitrary traits and roundtrip fuzz tests.
///
/// If you prefer to manually implement the arbitrary traits, you can still use the [`add_arbitrary_tests()`] function to add arbitrary fuzz tests.
///
/// Example usage:
/// * `#[reth_codec(rlp)]`: will implement `derive_arbitrary(rlp)` or `derive_arbitrary(compact, rlp)`, if `compact` is the `reth_codec`.
/// * `#[reth_codec(no_arbitrary)]`: will skip `derive_arbitrary` (both trait implementations and tests)
#[proc_macro_attribute]
#[rustfmt::skip]
#[allow(unreachable_code)]
pub fn reth_codec(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let with_zstd = args.clone().into_iter().any(|tk| tk.to_string() == "zstd");
    let without_arbitrary = args.clone().into_iter().any(|tk| tk.to_string() == "no_arbitrary");
    
    let compact = if with_zstd {
        quote! {
            #[derive(CompactZstd)]
            #ast
        }
        .into()
    } else {
        quote! {
            #[derive(Compact)]
            #ast
        }
        .into()
    };

    if without_arbitrary {
        return compact
    }

    let mut args = args.into_iter().collect::<Vec<_>>();
    args.push(TokenTree::Ident(proc_macro::Ident::new("compact", proc_macro::Span::call_site())));

    derive_arbitrary(TokenStream::from_iter(args), compact)
}

/// Adds `Arbitrary` imports into scope and derives the struct/enum.
///
/// If `compact` or `rlp` is passed to `derive_arbitrary`, there will be proptest roundtrip tests
/// generated. An integer value passed will limit the number of proptest cases generated (default:
/// 256).
///
/// Examples:
/// * `#[derive_arbitrary]`: will derive arbitrary with no tests.
/// * `#[derive_arbitrary(rlp)]`: will derive arbitrary and generate rlp roundtrip proptests.
/// * `#[derive_arbitrary(rlp, 10)]`: will derive arbitrary and generate rlp roundtrip proptests.
///   Limited to 10 cases.
/// * `#[derive_arbitrary(compact, rlp)]`. will derive arbitrary and generate rlp and compact
///   roundtrip proptests.
#[proc_macro_attribute]
pub fn derive_arbitrary(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let tests = arbitrary::maybe_generate_tests(args, &ast);

    // Avoid duplicate names
    let arb_import = format_ident!("{}Arbitrary", ast.ident);

    quote! {
        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::Arbitrary as #arb_import;

        #[cfg_attr(any(test, feature = "arbitrary"), derive(#arb_import))]
        #ast

        #tests
    }
    .into()
}

/// To be used for types that implement `Arbitrary` manually. See [`derive_arbitrary()`] for more.
#[proc_macro_attribute]
pub fn add_arbitrary_tests(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let tests = arbitrary::maybe_generate_tests(args, &ast);
    quote! {
        #ast
        #tests
    }
    .into()
}
