use proc_macro::{self, TokenStream};
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_attribute]
#[rustfmt::skip]
#[clippy::skip]
#[allow(unreachable_code)]
pub fn main_codec(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "scale")]
    return use_scale(args, input);

    #[cfg(feature = "no_codec")]
    return no_codec(args, input);

    #[cfg(feature = "no_codec")]
    return use_postcard(args, input);
}

#[proc_macro_attribute]
pub fn use_scale(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);
    let compactable_types = ["u8", "u16", "u32", "i32", "i64", "u64", "f32", "f64"];

    match &mut ast.data {
        syn::Data::Struct(ref mut data) => {
            match &mut data.fields {
                syn::Fields::Named(fields) => {
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
                _ => (),
            }
            return quote! {
                #[derive(parity_scale_codec::Encode, parity_scale_codec::Decode)]
                #ast
            }
            .into()
        }
        _ => unreachable!(),
    }
}

#[proc_macro_attribute]
pub fn use_postcard(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);
    match &mut ast.data {
        syn::Data::Struct(_) => {
            return quote! {
                #[derive(serde::Serialize, serde::Deserialize)]
                #ast
            }
            .into()
        }
        _ => unreachable!(),
    }
}

#[proc_macro_attribute]
pub fn no_codec(_args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    return quote! { #ast }.into()
}
