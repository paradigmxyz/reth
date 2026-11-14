//! Inert replacement for `metrics_derive::Metrics` in fuzz builds.
//!
//! Keeps the static/dynamic constructor, description, and `Debug` APIs without recorder access,
//! label evaluation, or process-global synchronization. Skipped fields still use `Default`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Error, Expr, Fields, LitBool, LitStr};

/// Derives no-op metric handles for fuzzing.
#[proc_macro_derive(Metrics, attributes(metrics, metric))]
pub fn derive_metrics(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive(&input).unwrap_or_else(|err| err.to_compile_error().into())
}

fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let mut scope = None;
    let mut dynamic = false;
    for attr in input.attrs.iter().filter(|attr| attr.path().is_ident("metrics")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("dynamic") {
                dynamic = meta.value()?.parse::<LitBool>()?.value;
            } else if meta.path.is_ident("scope") {
                scope = Some(meta.value()?.parse::<LitStr>()?);
            } else if meta.path.is_ident("separator") {
                meta.value()?.parse::<LitStr>()?;
            } else if meta.path.is_ident("labels") {
                // Labels are intentionally not evaluated in fuzz builds.
                meta.value()?.parse::<Expr>()?;
            } else {
                return Err(meta.error("unsupported attribute entry"));
            }
            Ok(())
        })?;
    }
    if dynamic == scope.is_some() {
        return Err(Error::new_spanned(
            input,
            "either `scope = ..` or `dynamic = true` must be set",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(input, "only structs are supported"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(input, "only named fields are supported"));
    };
    let mut inits = Vec::with_capacity(fields.named.len());
    for field in &fields.named {
        let mut skip = false;
        for attr in field.attrs.iter().filter(|attr| attr.path().is_ident("metric")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    skip = true;
                } else if meta.path.is_ident("rename") || meta.path.is_ident("describe") {
                    meta.value()?.parse::<LitStr>()?;
                } else if meta.path.is_ident("labels") {
                    meta.value()?.parse::<Expr>()?;
                } else {
                    return Err(meta.error("unsupported attribute entry"));
                }
                Ok(())
            })?;
        }
        let name = &field.ident;
        let ty = &field.ty;
        let init = if skip {
            quote! { ::core::default::Default::default() }
        } else {
            quote! { <#ty>::noop() }
        };
        inits.push(quote! { #name: #init });
    }
    let ty = &input.ident;
    let name = ty.to_string();
    let vis = &input.vis;
    let constructors = if dynamic {
        quote! {
            impl #ty {
                /// Creates inert metrics; the scope is ignored.
                #vis fn new(_: &str) -> Self {
                    Self { #(#inits),* }
                }

                /// Creates inert metrics without converting the labels.
                #vis fn new_with_labels(_: &str, _: impl metrics::IntoLabels) -> Self {
                    Self { #(#inits),* }
                }

                /// Does nothing in fuzz builds.
                #vis fn describe(_: &str) {}
            }
        }
    } else {
        quote! {
            impl ::core::default::Default for #ty {
                fn default() -> Self {
                    Self { #(#inits),* }
                }
            }

            impl #ty {
                /// Creates inert metrics without converting the labels.
                #vis fn new_with_labels(_: impl metrics::IntoLabels) -> Self {
                    Self::default()
                }

                /// Does nothing in fuzz builds.
                #vis fn describe() {}

                /// Does nothing in fuzz builds.
                #vis fn force_describe() {}
            }
        }
    };
    Ok(quote! {
        #constructors

        impl ::core::fmt::Debug for #ty {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(#name).finish()
            }
        }
    }
    .into())
}
