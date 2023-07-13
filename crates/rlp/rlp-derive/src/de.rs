use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Result};

use crate::utils::{attributes_include, field_ident, is_optional, parse_struct, EMPTY_STRING_CODE};

pub(crate) fn impl_decodable(ast: &syn::DeriveInput) -> Result<TokenStream> {
    let body = parse_struct(ast, "RlpDecodable")?;

    let fields = body.fields.iter().enumerate();

    let supports_trailing_opt = attributes_include(&ast.attrs, "trailing");

    let mut encountered_opt_item = false;
    let mut stmts = Vec::with_capacity(body.fields.len());
    for (i, field) in fields {
        let is_opt = is_optional(field);
        if is_opt {
            if !supports_trailing_opt {
                return Err(Error::new_spanned(field, "Optional fields are disabled. Add `#[rlp(trailing)]` attribute to the struct in order to enable"));
            }
            encountered_opt_item = true;
        } else if encountered_opt_item && !attributes_include(&field.attrs, "default") {
            return Err(Error::new_spanned(
                field,
                "All subsequent fields must be either optional or default.",
            ))
        }

        stmts.push(decodable_field(i, field, is_opt));
    }

    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let impl_block = quote! {
        impl #impl_generics reth_rlp::Decodable for #name #ty_generics #where_clause {
            fn decode(mut buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
                let b = &mut &**buf;
                let rlp_head = reth_rlp::Header::decode(b)?;

                if !rlp_head.list {
                    return Err(reth_rlp::DecodeError::UnexpectedString);
                }

                let started_len = b.len();
                let this = Self {
                    #(#stmts)*
                };

                let consumed = started_len - b.len();
                if consumed != rlp_head.payload_length {
                    return Err(reth_rlp::DecodeError::ListLengthMismatch {
                        expected: rlp_head.payload_length,
                        got: consumed,
                    });
                }

                *buf = *b;

                Ok(this)
            }
        }
    };

    Ok(quote! {
        const _: () = {
            extern crate reth_rlp;
            #impl_block
        };
    })
}

pub(crate) fn impl_decodable_wrapper(ast: &syn::DeriveInput) -> Result<TokenStream> {
    let body = parse_struct(ast, "RlpDecodableWrapper")?;

    assert_eq!(
        body.fields.iter().count(),
        1,
        "#[derive(RlpDecodableWrapper)] is only defined for structs with one field."
    );

    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let impl_block = quote! {
        impl #impl_generics reth_rlp::Decodable for #name #ty_generics #where_clause {
            fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
                Ok(Self(reth_rlp::Decodable::decode(buf)?))
            }
        }
    };

    Ok(quote! {
        const _: () = {
            extern crate reth_rlp;
            #impl_block
        };
    })
}

fn decodable_field(index: usize, field: &syn::Field, is_opt: bool) -> TokenStream {
    let ident = field_ident(index, field);

    if attributes_include(&field.attrs, "default") {
        quote! { #ident: Default::default(), }
    } else if is_opt {
        quote! {
            #ident: if started_len - b.len() < rlp_head.payload_length {
                if b.first().map(|b| *b == #EMPTY_STRING_CODE).unwrap_or_default() {
                    bytes::Buf::advance(b, 1);
                    None
                } else {
                    Some(reth_rlp::Decodable::decode(b)?)
                }
            } else {
                None
            },
        }
    } else {
        quote! { #ident: reth_rlp::Decodable::decode(b)?, }
    }
}
