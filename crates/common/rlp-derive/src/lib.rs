//! Derive macro for `#[derive(RlpEncodable, RlpDecodable)]`.
//!
//! For example of usage see `./tests/rlp.rs`.
//!
//! This library also supports up to 1 `#[rlp(default)]` in a struct,
//! which is similar to [`#[serde(default)]`](https://serde.rs/field-attrs.html#default)
//! with the caveat that we use the `Default` value if
//! the field deserialization fails, as we don't serialize field
//! names and there is no way to tell if it is present or not.

extern crate proc_macro;

mod de;
mod en;

use de::*;
use en::*;
use proc_macro::TokenStream;

#[proc_macro_derive(RlpEncodable, attributes(rlp))]
pub fn encodable(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    let gen = impl_encodable(&ast);
    gen.into()
}

#[proc_macro_derive(RlpEncodableWrapper, attributes(rlp))]
pub fn encodable_wrapper(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    let gen = impl_encodable_wrapper(&ast);
    gen.into()
}

#[proc_macro_derive(RlpMaxEncodedLen, attributes(rlp))]
pub fn max_encoded_len(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    let gen = impl_max_encoded_len(&ast);
    gen.into()
}

#[proc_macro_derive(RlpDecodable, attributes(rlp))]
pub fn decodable(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    let gen = impl_decodable(&ast);
    gen.into()
}

#[proc_macro_derive(RlpDecodableWrapper, attributes(rlp))]
pub fn decodable_wrapper(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    let gen = impl_decodable_wrapper(&ast);
    gen.into()
}
