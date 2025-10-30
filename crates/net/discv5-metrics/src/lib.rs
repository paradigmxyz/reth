//! Discv5 peers per `KBucket` metric derive macro.

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
use syn::{parse_macro_input, parse_quote, token::Brace, ImplItemFn, ItemImpl, ItemStruct};

// Awesome
#[proc_macro_attribute]
pub fn kbuckets_metrics(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input struct
    let input = parse_macro_input!(item as ItemStruct);
    let struct_token = input.struct_token;
    let vis = input.vis;
    let name = input.ident;

    const MAX_KBUCKET_INDEX: usize = 255;
    const FIELD_PREFIX: &str = "kbucket";

    let peer_counts_arg = format_ident!("peer_counts");

    // Generate 256 fields
    let fields: TokenStream2 = (0..=MAX_KBUCKET_INDEX)
        .map(|i| {
            let docs = format!("Total peers in KBucket at index {i}");
            let field = format_ident!("{FIELD_PREFIX}_{i}");
            quote! {
                #[doc = #docs]
                pub #field: Gauge
            }
        })
        .fold(TokenStream2::new(), |acc, ts| quote! { #acc #ts, });

    let set_peer_count = (0..=MAX_KBUCKET_INDEX).map(|i| {
        let field = format_ident!("{FIELD_PREFIX}_{i}");
        quote! {
            if let Some(count) = #peer_counts_arg.next() {
                self.#field.set(count as f64)
            }
        }
    });

    let ty_def = quote! {
        /// Tracks peers per kbucket.
        #[derive(Clone, Metrics)]
        #[metrics(scope = "discv5")]
        #vis #struct_token #name {
            #fields
        }
    };

    let method: ImplItemFn = parse_quote! {
        /// Sets current total number of connected peers in each kbucket. Takes an iterator
        /// over peer count in kbuckets starting from index bucket at index 0 to
        /// [`MAX_KBUCKET_INDEX`].
        pub fn set_total_peers_by_kbucket(&self, peer_counts: Vec<usize>) {
            let mut peer_counts = peer_counts.into_iter();
                #(#set_peer_count)*
        }
    };

    let impl_block: ItemImpl = ItemImpl {
        self_ty: Box::new(parse_quote!(#name)),
        items: vec![syn::ImplItem::Fn(method)],
        impl_token: parse_quote!(impl),
        generics: parse_quote!(<>),
        trait_: None,
        brace_token: Brace::default(),
        attrs: vec![],
        defaultness: None,
        unsafety: None,
    };

    let impl_block: TokenStream2 = quote!(#impl_block);

    let stream = quote! {
        #ty_def
        #impl_block
    };

    TokenStream::from(stream)
}
