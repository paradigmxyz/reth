use proc_macro2::TokenStream;
use quote::quote;

use crate::utils::has_attribute;

pub(crate) fn impl_decodable(ast: &syn::DeriveInput) -> TokenStream {
    let body = if let syn::Data::Struct(s) = &ast.data {
        s
    } else {
        panic!("#[derive(RlpDecodable)] is only defined for structs.");
    };

    let stmts: Vec<_> =
        body.fields.iter().enumerate().map(|(i, field)| decodable_field(i, field)).collect();
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

    quote! {
        const _: () = {
            extern crate reth_rlp;
            #impl_block
        };
    }
}

pub(crate) fn impl_decodable_wrapper(ast: &syn::DeriveInput) -> TokenStream {
    let body = if let syn::Data::Struct(s) = &ast.data {
        s
    } else {
        panic!("#[derive(RlpEncodableWrapper)] is only defined for structs.");
    };

    assert_eq!(
        body.fields.iter().count(),
        1,
        "#[derive(RlpEncodableWrapper)] is only defined for structs with one field."
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

    quote! {
        const _: () = {
            extern crate reth_rlp;
            #impl_block
        };
    }
}

fn decodable_field(index: usize, field: &syn::Field) -> TokenStream {
    let id = if let Some(ident) = &field.ident {
        quote! { #ident }
    } else {
        let index = syn::Index::from(index);
        quote! { #index }
    };

    if has_attribute(field, "default") {
        quote! { #id: Default::default(), }
    } else {
        quote! { #id: reth_rlp::Decodable::decode(b)?, }
    }
}
