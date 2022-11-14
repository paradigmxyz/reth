extern crate proc_macro2;
use proc_macro::{self, TokenStream};
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput};

mod generator;
use generator::*;

// Helper Alias type
type IsCompact = bool;
// Helper Alias type
type FieldName = String;
// Helper Alias type
type FieldType = String;
// Helper Alias type
type FieldList = Vec<(FieldName, FieldType, IsCompact)>;

/// Derives the [`Compact`] trait and its from/to implementations.
pub fn derive(input: TokenStream) -> TokenStream {
    let mut output = quote! {};

    let DeriveInput { ident, data, .. } = parse_macro_input!(input);
    let fields = get_fields(&data);

    output.extend(generate_flag_struct(&ident, &fields));
    output.extend(generate_from_to(&ident, &fields));
    output.into()
}

/// Given a list of fields on a struct, extract their fields and types.
pub fn get_fields(data: &Data) -> FieldList {
    let mut named_fields = vec![];
    if let syn::Data::Struct(data) = data {
        if let syn::Fields::Named(ref fields) = data.fields {
            for field in &fields.named {
                if let syn::Type::Path(ref path) = field.ty {
                    let segments = &path.path.segments;
                    if !segments.is_empty() {
                        let mut ftype = String::new();
                        for (index, segment) in segments.iter().enumerate() {
                            ftype.push_str(&segment.ident.to_string());
                            if index < segments.len() - 1 {
                                ftype.push_str("::");
                            }
                        }
                        let should_compact = !not_flag_type(&ftype) ||
                            field.attrs.iter().any(|attr| {
                                attr.path.segments.iter().any(|path| path.ident == "maybe_zero")
                            });

                        named_fields.push((
                            field.ident.as_ref().expect("qed").to_string(),
                            ftype,
                            should_compact,
                        ));
                    }
                }
            }
            assert_eq!(named_fields.len(), fields.named.len());
        }
    }
    named_fields
}

/// Given the field type in a string format, return the amount of bits necessary to save its maximum
/// length.
pub fn get_bit_size(ftype: &str) -> u8 {
    if ftype == "u64" {
        return 4
    } else if ftype == "TxType" {
        return 2
    } else if ftype == "bool" || ftype == "Option" {
        return 1
    }
    6
}

/// Given the field type in a string format, checks if it's type that shouldn't be added to the
/// StructFlags.
pub fn not_flag_type(ftype: &str) -> bool {
    let known = ["H256", "H160", "Address", "Bloom"];
    known.contains(&ftype) || ftype.starts_with("Vec")
}

/// Given the field name in a string format, returns various [`Ident`] necessary to generate code
/// with [`quote`].
pub fn get_field_idents(name: &str) -> (Ident, Ident, Ident, Ident) {
    let name = format_ident!("{name}");
    let set_len_method = format_ident!("set_{name}_len");
    let slice = format_ident!("{name}_slice");
    let len = format_ident!("{name}_len");
    (name, set_len_method, slice, len)
}
