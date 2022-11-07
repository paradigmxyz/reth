extern crate proc_macro2;
use proc_macro::{self, TokenStream};
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput};

mod generator;
use generator::*;

type IsCompact = bool;
type FieldName = String;
type FieldType = String;
type FieldList = Vec<(FieldName, FieldType, IsCompact)>;

pub fn derive(input: TokenStream) -> TokenStream {
    let DeriveInput { ident, data, .. } = parse_macro_input!(input);

    let fields = get_fields(&data);

    let mut output = quote! {};
    output.extend(generate_flag_struct(&ident, &fields));
    output.extend(generate_from_to(&ident, &fields));
    output.into()
}

pub fn get_fields(data: &Data) -> FieldList {
    let mut named_fields = vec![];
    if let syn::Data::Struct(data) = data {
        if let syn::Fields::Named(ref fields) = data.fields {
            for field in &fields.named {
                if let syn::Type::Path(ref path) = field.ty {
                    let segments = &path.path.segments;
                    if !segments.is_empty() {
                        // TODO: attribute that makes it raw if requested. Which means it wont be
                        // part of the flags
                        let mut ftype = String::new();
                        for (index, s) in segments.iter().enumerate() {
                            ftype.push_str(&s.ident.to_string());
                            if index < segments.len() - 1 {
                                ftype.push_str("::");
                            }
                        }

                        named_fields.push((
                            field.ident.as_ref().expect("qed").to_string(),
                            ftype,
                            true,
                        ));
                    }
                }
            }
            assert_eq!(named_fields.len(), fields.named.len());
        }
    }
    named_fields
}

pub fn get_bit_size(ftype: &str) -> u8 {
    if ftype == "u64" {
        return 4
    }
    if ftype == "U256" {
        return 6
    }
    6
}

pub fn is_hash_type(ftype: &str) -> bool {
    let known = ["H256", "H160", "Address"];
    known.contains(&ftype)
}

pub fn get_field_idents(name: &str) -> (Ident, Ident, Ident, Ident) {
    let name = format_ident!("{name}");
    let set_len_method = format_ident!("set_{name}_len");
    let slice = format_ident!("{name}_slice");
    let len = format_ident!("{name}_len");
    (name, set_len_method, slice, len)
}
