//! Derive macros for the Compact codec traits.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![allow(unreachable_pub, missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use proc_macro::{self, TokenStream, TokenTree};
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

/// This code implements the main codec. If the codec supports it, it will also provide the [derive_arbitrary()] function, which automatically implements arbitrary traits and roundtrip fuzz tests.
///
/// If you prefer to manually implement the arbitrary traits, you can still use the [add_arbitrary_tests()] function to add arbitrary fuzz tests.
///
/// Example usage:
/// * `#[main_codec(rlp)]`: will implement `derive_arbitrary(rlp)` or `derive_arbitrary(compact, rlp)`, if `compact` is the `main_codec`.
/// * `#[main_codec(no_arbitrary)]`: will skip `derive_arbitrary` (both trait implementations and tests)
#[proc_macro_attribute]
#[rustfmt::skip]
#[allow(unreachable_code)]
pub fn main_codec(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "compact")]
    return use_compact(args, input);

    #[cfg(feature = "scale")]
    return use_scale(args, input);

    #[cfg(feature = "postcard")]
    return use_postcard(args, input);

    #[cfg(feature = "no_codec")]
    return no_codec(args, input);

    // no features
    no_codec(args, input)
}

#[proc_macro_attribute]
pub fn use_scale(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);
    let compactable_types = ["u8", "u16", "u32", "i32", "i64", "u64", "f32", "f64"];

    if let syn::Data::Struct(ref mut data) = &mut ast.data {
        if let syn::Fields::Named(fields) = &mut data.fields {
            for field in fields.named.iter_mut() {
                if let syn::Type::Path(ref path) = field.ty {
                    if !path.path.segments.is_empty() {
                        let _type = format!("{}", path.path.segments[0].ident);
                        if compactable_types.contains(&_type.as_str()) {
                            field.attrs.push(syn::parse_quote! {
                                #[codec(compact)]
                            });
                        }
                    }
                }
            }
        }
    }

    quote! {
        #[derive(parity_scale_codec::Encode, parity_scale_codec::Decode, serde::Serialize, serde::Deserialize)]
        #ast
    }
    .into()
}

#[proc_macro_attribute]
pub fn use_postcard(_args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    quote! {
        #[derive(serde::Serialize, serde::Deserialize)]
        #ast
    }
    .into()
}

#[proc_macro_attribute]
pub fn use_compact(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let with_zstd = args.clone().into_iter().any(|tk| tk.to_string() == "zstd");

    let compact = if with_zstd {
        quote! {
            #[derive(CompactZstd, serde::Serialize, serde::Deserialize)]
            #ast
        }
        .into()
    } else {
        quote! {
            #[derive(Compact, serde::Serialize, serde::Deserialize)]
            #ast
        }
        .into()
    };

    if let Some(first_arg) = args.clone().into_iter().next() {
        if first_arg.to_string() == "no_arbitrary" {
            return compact
        }
    }

    let mut args = args.into_iter().collect::<Vec<_>>();
    args.push(TokenTree::Ident(proc_macro::Ident::new("compact", proc_macro::Span::call_site())));

    derive_arbitrary(TokenStream::from_iter(args), compact)
}

/// Adds `Arbitrary` and `proptest::Arbitrary` imports into scope and derives the struct/enum.
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
    let prop_import = format_ident!("{}PropTestArbitrary", ast.ident);
    let arb_import = format_ident!("{}Arbitrary", ast.ident);

    quote! {
        #[cfg(any(test, feature = "arbitrary"))]
        use proptest_derive::Arbitrary as #prop_import;

        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::Arbitrary as #arb_import;

        #[cfg_attr(any(test, feature = "arbitrary"), derive(#prop_import, #arb_import))]
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

#[proc_macro_attribute]
pub fn no_codec(_args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    quote! { #ast }.into()
}
