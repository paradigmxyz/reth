use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Generics};

mod generator;
use generator::*;

mod enums;
use enums::*;

mod flags;
use flags::*;

mod structs;
use structs::*;

// Helper Alias type
type IsCompact = bool;
// Helper Alias type
type FieldName = String;
// Helper Alias type
type FieldType = String;
/// `Compact` has alternative functions that can be used as a workaround for type
/// specialization of fixed sized types.
///
/// Example: `Vec<B256>` vs `Vec<U256>`. The first does not
/// require the len of the element, while the latter one does.
type UseAlternative = bool;
// Helper Alias type
type StructFieldDescriptor = (FieldName, FieldType, IsCompact, UseAlternative);
// Helper Alias type
type FieldList = Vec<FieldTypes>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FieldTypes {
    StructField(StructFieldDescriptor),
    EnumVariant(String),
    EnumUnnamedField((FieldType, UseAlternative)),
}

/// Derives the `Compact` trait and its from/to implementations.
pub fn derive(input: TokenStream, is_zstd: bool) -> TokenStream {
    let mut output = quote! {};

    let DeriveInput { ident, data, generics, .. } = parse_macro_input!(input);

    let has_lifetime = has_lifetime(&generics);

    let fields = get_fields(&data);
    output.extend(generate_flag_struct(&ident, has_lifetime, &fields, is_zstd));
    output.extend(generate_from_to(&ident, has_lifetime, &fields, is_zstd));
    output.into()
}

pub fn has_lifetime(generics: &Generics) -> bool {
    generics.lifetimes().next().is_some()
}

/// Given a list of fields on a struct, extract their fields and types.
pub fn get_fields(data: &Data) -> FieldList {
    let mut fields = vec![];

    match data {
        Data::Struct(data) => match data.fields {
            syn::Fields::Named(ref data_fields) => {
                for field in &data_fields.named {
                    load_field(field, &mut fields, false);
                }
                assert_eq!(fields.len(), data_fields.named.len(), "get_fields");
            }
            syn::Fields::Unnamed(ref data_fields) => {
                assert_eq!(
                    data_fields.unnamed.len(),
                    1,
                    "Compact only allows one unnamed field. Consider making it a struct."
                );
                load_field(&data_fields.unnamed[0], &mut fields, false);
            }
            syn::Fields::Unit => todo!(),
        },
        Data::Enum(data) => {
            for variant in &data.variants {
                fields.push(FieldTypes::EnumVariant(variant.ident.to_string()));

                match &variant.fields {
                    syn::Fields::Named(_) => {
                        panic!("Not allowed to have Enum Variants with multiple named fields. Make it a struct instead.")
                    }
                    syn::Fields::Unnamed(data_fields) => {
                        assert_eq!(
                            data_fields.unnamed.len(),
                            1,
                            "Compact only allows one unnamed field. Consider making it a struct."
                        );
                        load_field(&data_fields.unnamed[0], &mut fields, true);
                    }
                    syn::Fields::Unit => (),
                }
            }
        }
        Data::Union(_) => todo!(),
    }

    fields
}

fn load_field(field: &syn::Field, fields: &mut FieldList, is_enum: bool) {
    match field.ty {
        syn::Type::Reference(ref reference) => match &*reference.elem {
            syn::Type::Path(path) => {
                load_field_from_segments(&path.path.segments, is_enum, fields, field)
            }
            _ => unimplemented!("{:?}", &field.ident),
        },
        syn::Type::Path(ref path) => {
            load_field_from_segments(&path.path.segments, is_enum, fields, field)
        }
        _ => unimplemented!("{:?}", &field.ident),
    }
}

fn load_field_from_segments(
    segments: &syn::punctuated::Punctuated<syn::PathSegment, syn::token::PathSep>,
    is_enum: bool,
    fields: &mut Vec<FieldTypes>,
    field: &syn::Field,
) {
    if !segments.is_empty() {
        let mut ftype = String::new();

        let mut use_alt_impl: UseAlternative = false;

        for (index, segment) in segments.iter().enumerate() {
            ftype.push_str(&segment.ident.to_string());
            if index < segments.len() - 1 {
                ftype.push_str("::");
            }

            use_alt_impl = should_use_alt_impl(&ftype, segment);
        }

        if is_enum {
            fields.push(FieldTypes::EnumUnnamedField((ftype.to_string(), use_alt_impl)));
        } else {
            let should_compact = is_flag_type(&ftype) ||
                field.attrs.iter().any(|attr| {
                    attr.path().segments.iter().any(|path| path.ident == "maybe_zero")
                });

            fields.push(FieldTypes::StructField((
                field.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                ftype,
                should_compact,
                use_alt_impl,
            )));
        }
    }
}

/// Since there's no impl specialization in rust stable atm, once we find we have a
/// Vec/Option we try to find out if it's a Vec/Option of a fixed size data type, e.g. `Vec<B256>`.
///
/// If so, we use another impl to code/decode its data.
fn should_use_alt_impl(ftype: &String, segment: &syn::PathSegment) -> bool {
    if *ftype == "Vec" || *ftype == "Option" {
        if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
            if let Some(syn::GenericArgument::Type(syn::Type::Path(arg_path))) = args.args.last() {
                if let (Some(path), 1) =
                    (arg_path.path.segments.first(), arg_path.path.segments.len())
                {
                    if [
                        "B256",
                        "Address",
                        "Address",
                        "Bloom",
                        "TxHash",
                        "BlockHash",
                        "CompactPlaceholder",
                    ]
                    .contains(&path.ident.to_string().as_str())
                    {
                        return true
                    }
                }
            }
        }
    }
    false
}

/// Given the field type in a string format, return the amount of bits necessary to save its maximum
/// length.
pub fn get_bit_size(ftype: &str) -> u8 {
    match ftype {
        "TransactionKind" | "TxKind" | "bool" | "Option" | "Signature" => 1,
        "TxType" => 2,
        "u64" | "BlockNumber" | "TxNumber" | "ChainId" | "NumTransactions" => 4,
        "u128" => 5,
        "U256" => 6,
        _ => 0,
    }
}

/// Given the field type in a string format, checks if its type should be added to the
/// `StructFlags`.
pub fn is_flag_type(ftype: &str) -> bool {
    get_bit_size(ftype) > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use similar_asserts::assert_eq;
    use syn::parse2;

    #[test]
    fn gen() {
        let f_struct = quote! {
             #[derive(Debug, PartialEq, Clone)]
             pub struct TestStruct {
                 f_u64: u64,
                 f_u256: U256,
                 f_bool_t: bool,
                 f_bool_f: bool,
                 f_option_none: Option<U256>,
                 f_option_some: Option<B256>,
                 f_option_some_u64: Option<u64>,
                 f_vec_empty: Vec<U256>,
                 f_vec_some: Vec<Address>,
             }
        };

        // Generate code that will impl the `Compact` trait.
        let mut output = quote! {};
        let DeriveInput { ident, data, .. } = parse2(f_struct).unwrap();
        let fields = get_fields(&data);
        output.extend(generate_flag_struct(&ident, false, &fields, false));
        output.extend(generate_from_to(&ident, false, &fields, false));

        // Expected output in a TokenStream format. Commas matter!
        let should_output = quote! {
            impl TestStruct {
                #[doc = "Used bytes by [`TestStructFlags`]"]
                pub const fn bitflag_encoded_bytes() -> usize {
                    2u8 as usize
                }
            }

            pub use TestStruct_flags::TestStructFlags;

            #[allow(non_snake_case)]
            mod TestStruct_flags {
                use bytes::Buf;
                use modular_bitfield::prelude::*;
                #[doc = "Fieldset that facilitates compacting the parent type. Used bytes: 2 | Unused bits: 1"]
                #[bitfield]
                #[derive(Clone, Copy, Debug, Default)]
                pub struct TestStructFlags {
                    pub f_u64_len: B4,
                    pub f_u256_len: B6,
                    pub f_bool_t_len: B1,
                    pub f_bool_f_len: B1,
                    pub f_option_none_len: B1,
                    pub f_option_some_len: B1,
                    pub f_option_some_u64_len: B1,
                    #[skip]
                    unused: B1,
                }
                impl TestStructFlags {
                    #[doc = r" Deserializes this fieldset and returns it, alongside the original slice in an advanced position."]
                    pub fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                        (
                            TestStructFlags::from_bytes([buf.get_u8(), buf.get_u8(),]),
                            buf
                        )
                    }
                }
            }
            #[cfg(test)]
            #[allow(dead_code)]
            #[test_fuzz::test_fuzz]
            fn fuzz_test_test_struct(obj: TestStruct) {
                let mut buf = vec![];
                let len = obj.clone().to_compact(&mut buf);
                let (same_obj, buf) = TestStruct::from_compact(buf.as_ref(), len);
                assert_eq!(obj, same_obj);
            }
            #[test]
            pub fn fuzz_test_struct() {
                fuzz_test_test_struct(TestStruct::default())
            }
            impl Compact for TestStruct {
                fn to_compact<B>(&self, buf: &mut B) -> usize where B: bytes::BufMut + AsMut<[u8]> {
                    let mut flags = TestStructFlags::default();
                    let mut total_length = 0;
                    let mut buffer = bytes::BytesMut::new();
                    let f_u64_len = self.f_u64.to_compact(&mut buffer);
                    flags.set_f_u64_len(f_u64_len as u8);
                    let f_u256_len = self.f_u256.to_compact(&mut buffer);
                    flags.set_f_u256_len(f_u256_len as u8);
                    let f_bool_t_len = self.f_bool_t.to_compact(&mut buffer);
                    flags.set_f_bool_t_len(f_bool_t_len as u8);
                    let f_bool_f_len = self.f_bool_f.to_compact(&mut buffer);
                    flags.set_f_bool_f_len(f_bool_f_len as u8);
                    let f_option_none_len = self.f_option_none.to_compact(&mut buffer);
                    flags.set_f_option_none_len(f_option_none_len as u8);
                    let f_option_some_len = self.f_option_some.specialized_to_compact(&mut buffer);
                    flags.set_f_option_some_len(f_option_some_len as u8);
                    let f_option_some_u64_len = self.f_option_some_u64.to_compact(&mut buffer);
                    flags.set_f_option_some_u64_len(f_option_some_u64_len as u8);
                    let f_vec_empty_len = self.f_vec_empty.to_compact(&mut buffer);
                    let f_vec_some_len = self.f_vec_some.specialized_to_compact(&mut buffer);
                    let flags = flags.into_bytes();
                    total_length += flags.len() + buffer.len();
                    buf.put_slice(&flags);
                    buf.put(buffer);
                    total_length
                }
                fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                    let (flags, mut buf) = TestStructFlags::from(buf);
                    let (f_u64, new_buf) = u64::from_compact(buf, flags.f_u64_len() as usize);
                    buf = new_buf;
                    let (f_u256, new_buf) = U256::from_compact(buf, flags.f_u256_len() as usize);
                    buf = new_buf;
                    let (f_bool_t, new_buf) = bool::from_compact(buf, flags.f_bool_t_len() as usize);
                    buf = new_buf;
                    let (f_bool_f, new_buf) = bool::from_compact(buf, flags.f_bool_f_len() as usize);
                    buf = new_buf;
                    let (f_option_none, new_buf) = Option::from_compact(buf, flags.f_option_none_len() as usize);
                    buf = new_buf;
                    let (f_option_some, new_buf) = Option::specialized_from_compact(buf, flags.f_option_some_len() as usize);
                    buf = new_buf;
                    let (f_option_some_u64, new_buf) = Option::from_compact(buf, flags.f_option_some_u64_len() as usize);
                    buf = new_buf;
                    let (f_vec_empty, new_buf) = Vec::from_compact(buf, buf.len());
                    buf = new_buf;
                    let (f_vec_some, new_buf) = Vec::specialized_from_compact(buf, buf.len());
                    buf = new_buf;
                    let obj = TestStruct {
                        f_u64: f_u64,
                        f_u256: f_u256,
                        f_bool_t: f_bool_t,
                        f_bool_f: f_bool_f,
                        f_option_none: f_option_none,
                        f_option_some: f_option_some,
                        f_option_some_u64: f_option_some_u64,
                        f_vec_empty: f_vec_empty,
                        f_vec_some: f_vec_some,
                    };
                    (obj, buf)
                }
            }
        };

        assert_eq!(
            syn::parse2::<syn::File>(output).unwrap(),
            syn::parse2::<syn::File>(should_output).unwrap()
        );
    }
}
