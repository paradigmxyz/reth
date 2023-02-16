use std::iter::Peekable;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Result};

use crate::utils::{attributes_include, field_ident, is_optional, parse_struct, EMPTY_STRING_CODE};

pub(crate) fn impl_encodable(ast: &syn::DeriveInput) -> Result<TokenStream> {
    let body = parse_struct(ast, "RlpEncodable")?;

    let mut fields = body
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| !attributes_include(&field.attrs, "skip"))
        .peekable();

    let supports_trailing_opt = attributes_include(&ast.attrs, "trailing");

    let mut encountered_opt_item = false;
    let mut length_stmts = Vec::with_capacity(body.fields.len());
    let mut stmts = Vec::with_capacity(body.fields.len());

    while let Some((i, field)) = fields.next() {
        let is_opt = is_optional(field);
        if is_opt {
            if !supports_trailing_opt {
                return Err(Error::new_spanned(field, "Optional fields are disabled. Add `#[rlp(trailing)]` attribute to the struct in order to enable"))
            }
            encountered_opt_item = true;
        } else if encountered_opt_item {
            return Err(Error::new_spanned(field, "All subsequent fields must be optional."))
        }

        length_stmts.push(encodable_length(i, field, is_opt, fields.clone()));
        stmts.push(encodable_field(i, field, is_opt, fields.clone()));
    }

    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let impl_block = quote! {
        trait E {
            fn rlp_header(&self) -> reth_rlp::Header;
        }

        impl #impl_generics E for #name #ty_generics #where_clause {
            fn rlp_header(&self) -> reth_rlp::Header {
                let mut rlp_head = reth_rlp::Header { list: true, payload_length: 0 };
                #(#length_stmts)*
                rlp_head
            }
        }

        impl #impl_generics reth_rlp::Encodable for #name #ty_generics #where_clause {
            fn length(&self) -> usize {
                let rlp_head = E::rlp_header(self);
                return reth_rlp::length_of_length(rlp_head.payload_length) + rlp_head.payload_length;
            }
            fn encode(&self, out: &mut dyn reth_rlp::BufMut) {
                E::rlp_header(self).encode(out);
                #(#stmts)*
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

pub(crate) fn impl_encodable_wrapper(ast: &syn::DeriveInput) -> Result<TokenStream> {
    let body = parse_struct(ast, "RlpEncodableWrapper")?;

    let ident = {
        let fields: Vec<_> = body.fields.iter().collect();
        if fields.len() == 1 {
            let field = fields.first().expect("fields.len() == 1; qed");
            field_ident(0, field)
        } else {
            panic!("#[derive(RlpEncodableWrapper)] is only defined for structs with one field.")
        }
    };

    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let impl_block = quote! {
        impl #impl_generics reth_rlp::Encodable for #name #ty_generics #where_clause {
            fn length(&self) -> usize {
                self.#ident.length()
            }
            fn encode(&self, out: &mut dyn reth_rlp::BufMut) {
                self.#ident.encode(out)
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

pub(crate) fn impl_max_encoded_len(ast: &syn::DeriveInput) -> Result<TokenStream> {
    let body = parse_struct(ast, "RlpMaxEncodedLen")?;

    let stmts: Vec<_> = body
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| !attributes_include(&field.attrs, "skip"))
        .map(|(index, field)| encodable_max_length(index, field))
        .collect();
    let name = &ast.ident;

    let impl_block = quote! {
        unsafe impl reth_rlp::MaxEncodedLen<{ reth_rlp::const_add(reth_rlp::length_of_length(#(#stmts)*), #(#stmts)*) }> for #name {}
        unsafe impl reth_rlp::MaxEncodedLenAssoc for #name {
            const LEN: usize = { reth_rlp::const_add(reth_rlp::length_of_length(#(#stmts)*), { #(#stmts)* }) };
        }
    };

    Ok(quote! {
        const _: () = {
            extern crate reth_rlp;
            #impl_block
        };
    })
}

fn encodable_length<'a>(
    index: usize,
    field: &syn::Field,
    is_opt: bool,
    mut remaining: Peekable<impl Iterator<Item = (usize, &'a syn::Field)>>,
) -> TokenStream {
    let ident = field_ident(index, field);

    if is_opt {
        let default = if remaining.peek().is_some() {
            let condition = remaining_opt_fields_some_condition(remaining);
            quote! { #condition as usize }
        } else {
            quote! { 0 }
        };

        quote! { rlp_head.payload_length += &self.#ident.as_ref().map(|val| reth_rlp::Encodable::length(val)).unwrap_or(#default); }
    } else {
        quote! { rlp_head.payload_length += reth_rlp::Encodable::length(&self.#ident); }
    }
}

fn encodable_max_length(index: usize, field: &syn::Field) -> TokenStream {
    let fieldtype = &field.ty;

    if index == 0 {
        quote! { <#fieldtype as reth_rlp::MaxEncodedLenAssoc>::LEN }
    } else {
        quote! { + <#fieldtype as reth_rlp::MaxEncodedLenAssoc>::LEN }
    }
}

fn encodable_field<'a>(
    index: usize,
    field: &syn::Field,
    is_opt: bool,
    mut remaining: Peekable<impl Iterator<Item = (usize, &'a syn::Field)>>,
) -> TokenStream {
    let ident = field_ident(index, field);

    if is_opt {
        let if_some_encode = quote! {
            if let Some(val) = self.#ident.as_ref() {
                reth_rlp::Encodable::encode(val, out)
            }
        };

        if remaining.peek().is_some() {
            let condition = remaining_opt_fields_some_condition(remaining);
            quote! {
                #if_some_encode
                else if #condition {
                    out.put_u8(#EMPTY_STRING_CODE);
                }
            }
        } else {
            quote! { #if_some_encode }
        }
    } else {
        quote! { reth_rlp::Encodable::encode(&self.#ident, out); }
    }
}

fn remaining_opt_fields_some_condition<'a>(
    remaining: impl Iterator<Item = (usize, &'a syn::Field)>,
) -> TokenStream {
    let conditions = remaining.map(|(index, field)| {
        let ident = field_ident(index, field);
        quote! { self.#ident.is_some() }
    });
    quote! { #(#conditions) ||* }
}
