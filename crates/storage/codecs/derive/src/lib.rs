//! Derive macros for the Compact codec traits.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(unreachable_pub, missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    bracketed,
    parse::{Parse, ParseStream},
    parse_macro_input, DeriveInput, Result, Token,
};

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

/// Generates tests for given type.
///
/// If `compact` or `rlp` is passed to `add_arbitrary_tests`, there will be proptest roundtrip tests
/// generated. An integer value passed will limit the number of proptest cases generated (default:
/// 256).
///
/// Examples:
/// * `#[add_arbitrary_tests]`: will derive arbitrary with no tests.
/// * `#[add_arbitrary_tests(rlp)]`: will derive arbitrary and generate rlp roundtrip proptests.
/// * `#[add_arbitrary_tests(rlp, 10)]`: will derive arbitrary and generate rlp roundtrip proptests.
///   Limited to 10 cases.
/// * `#[add_arbitrary_tests(compact, rlp)]`. will derive arbitrary and generate rlp and compact
///   roundtrip proptests.
#[proc_macro_attribute]
pub fn add_arbitrary_tests(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let tests =
        arbitrary::maybe_generate_tests(args, &ast.ident, &format_ident!("{}Tests", ast.ident));
    quote! {
        #ast
        #tests
    }
    .into()
}

struct GenerateTestsInput {
    args: TokenStream,
    ty: syn::Type,
    mod_name: syn::Ident,
}

impl Parse for GenerateTestsInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        input.parse::<Token![#]>()?;

        let args;
        bracketed!(args in input);

        let args = args.parse::<proc_macro2::TokenStream>()?;
        let ty = input.parse()?;

        input.parse::<Token![,]>()?;
        let mod_name = input.parse()?;

        Ok(Self { args: args.into(), ty, mod_name })
    }
}

/// Generates tests for given type based on passed parameters.
///
/// See `arbitrary::maybe_generate_tests` for more information.
///
/// Examples:
/// * `generate_tests!(#[rlp] MyType, MyTypeTests)`: will generate rlp roundtrip tests for `MyType`
///   in a module named `MyTypeTests`.
/// * `generate_tests!(#[compact, 10] MyType, MyTypeTests)`: will generate compact roundtrip tests
///   for `MyType` limited to 10 cases.
#[proc_macro]
pub fn generate_tests(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as GenerateTestsInput);

    arbitrary::maybe_generate_tests(input.args, &input.ty, &input.mod_name).into()
}
