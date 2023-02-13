use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, DataStruct, Error, Field, Meta, NestedMeta, Result, Type, TypePath};

pub(crate) fn parse_struct<'a>(
    ast: &'a syn::DeriveInput,
    derive_attr: &str,
) -> Result<&'a DataStruct> {
    if let syn::Data::Struct(s) = &ast.data {
        Ok(s)
    } else {
        Err(Error::new_spanned(
            ast,
            format!("#[derive({derive_attr})] is only defined for structs."),
        ))
    }
}

pub(crate) fn attributes_include(attrs: &[Attribute], attr_name: &str) -> bool {
    attrs.iter().any(|attr| {
        if attr.path.is_ident("rlp") {
            if let Ok(Meta::List(meta)) = attr.parse_meta() {
                if let Some(NestedMeta::Meta(meta)) = meta.nested.first() {
                    return meta.path().is_ident(attr_name)
                }
                return false
            } else {
            }
        }
        false
    })
}

pub(crate) fn is_optional(field: &Field) -> bool {
    if let Type::Path(TypePath { qself, path }) = &field.ty {
        qself.is_none() &&
            path.leading_colon.is_none() &&
            path.segments.len() == 1 &&
            path.segments.first().unwrap().ident == "Option"
    } else {
        false
    }
}

pub(crate) fn field_ident(index: usize, field: &syn::Field) -> TokenStream {
    if let Some(ident) = &field.ident {
        quote! { #ident }
    } else {
        let index = syn::Index::from(index);
        quote! { #index }
    }
}
