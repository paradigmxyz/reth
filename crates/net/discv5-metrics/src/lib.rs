//! Discv5 peers per KBucket metric derive macro.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![allow(unreachable_pub, missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemStruct};

// Awesome
#[proc_macro_attribute]
pub fn kbuckets_metrics(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input struct
    let input = parse_macro_input!(item as ItemStruct);
    let struct_token = input.struct_token;
    let vis = input.vis;
    let name = input.ident;

    const COUNT: usize = 256;
    const FIELD: &str = "kbucket";

    let index_arg = format_ident!("kbucket_index");
    let peers_arg = format_ident!("num_peers");

    // Generate 255 fields
    let fields: TokenStream2 = (0..COUNT)
        .map(|i| {
            let docs = format!("Total peers in KBucket at index {i}");
            let field = format_ident!("{FIELD}_{i}");
            quote! {
                #[doc = #docs]
                pub #field: Gauge
            }
        })
        .fold(TokenStream2::new(), |acc, ts| quote! { #acc #ts, });

    let set_peer_count = (0..COUNT)
        .rev() // more likely to find peers at further log2distance
        .map(|i| {
            let field = format_ident!("{FIELD}_{i}");
            quote! {
                if #index_arg == #i {
                    self.#field.set(#peers_arg as f64);
                    return
                }
            }
        });

    let expanded = quote! {
        /// Tracks peers per KBucket.
        #[derive(Clone, Metrics)]
        #[metrics(scope = "discv5")]
        #vis #struct_token #name {
            #fields
        }
    };

    let expanded2 = quote! {
        impl #name {
            /// Sets current total number of peers in KBucket of given index.
            pub fn set_total_peers_in(&mut self, #index_arg: usize, #peers_arg: usize) {
                #(#set_peer_count)*
            }
        }
    };

    let stream = quote! {
        #expanded
        #expanded2
    };

    TokenStream::from(stream)
}
