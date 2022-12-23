#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This crate provides [Metrics] derive macro

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

#[allow(unused_extern_crates)]
extern crate proc_macro;

mod expand;
mod loose_path;
mod metric;

/// The [Metrics] derive macro instruments all of the struct fields and
/// creates a [Default] implementation for the struct registering all of
/// the metrics.
///
/// Additionally, it creates a `describe` method on the struct, which
/// internally calls the describe statements for all metric fields.
#[proc_macro_derive(Metrics, attributes(scope, metric))]
pub fn derive_metrics(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::derive(&input).unwrap_or_else(|err| err.to_compile_error()).into()
}
