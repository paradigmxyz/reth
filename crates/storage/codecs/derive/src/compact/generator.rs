//! Code generator for the `Compact` trait.

use super::*;
use convert_case::{Case, Casing};

/// Generates code to implement the `Compact` trait for a data type.
pub fn generate_from_to(ident: &Ident, fields: &FieldList, is_zstd: bool) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let to_compact = generate_to_compact(fields, ident, is_zstd);
    let from_compact = generate_from_compact(fields, ident, is_zstd);

    let snake_case_ident = ident.to_string().to_case(Case::Snake);

    let fuzz = format_ident!("fuzz_test_{snake_case_ident}");
    let test = format_ident!("fuzz_{snake_case_ident}");

    // Build function
    quote! {

        #[cfg(test)]
        #[allow(dead_code)]
        #[test_fuzz::test_fuzz]
        fn #fuzz(obj: #ident)  {
            let mut buf = vec![];
            let len = obj.clone().to_compact(&mut buf);
            let (same_obj, buf) = #ident::from_compact(buf.as_ref(), len);
            assert_eq!(obj, same_obj);
        }

        #[test]
        pub fn #test() {
            #fuzz(#ident::default())
        }

        impl Compact for #ident {
            fn to_compact<B>(self, buf: &mut B) -> usize where B: bytes::BufMut + AsMut<[u8]> {
                let mut flags = #flags::default();
                let mut total_length = 0;
                #(#to_compact)*
                total_length
            }

            fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                let (flags, mut buf) = #flags::from(buf);
                #from_compact
            }
        }
    }
}

/// Generates code to implement the `Compact` trait method `to_compact`.
fn generate_from_compact(fields: &FieldList, ident: &Ident, is_zstd: bool) -> TokenStream2 {
    let mut lines = vec![];
    let mut known_types = vec!["B256", "Address", "Bloom", "Vec", "TxHash"];

    // Only types without `Bytes` should be added here. It's currently manually added, since
    // it's hard to figure out with derive_macro which types have Bytes fields.
    //
    // This removes the requirement of the field to be placed last in the struct.
    known_types.extend_from_slice(&[
        "TransactionKind",
        "AccessList",
        "Signature",
        "CheckpointBlockRange",
    ]);

    // let mut handle = FieldListHandler::new(fields);
    let is_enum = fields.iter().any(|field| matches!(field, FieldTypes::EnumVariant(_)));

    if is_enum {
        let enum_lines = EnumHandler::new(fields).generate_from(ident);

        // Builds the object instantiation.
        lines.push(quote! {
            let obj = match flags.variant() {
                #(#enum_lines)*
                _ => unreachable!()
            };
        });
    } else {
        let mut struct_handler = StructHandler::new(fields);
        lines.append(&mut struct_handler.generate_from(known_types.as_slice()));

        // Builds the object instantiation.
        if struct_handler.is_wrapper {
            lines.push(quote! {
                let obj = #ident(placeholder);
            });
        } else {
            let fields = fields.iter().filter_map(|field| {
                if let FieldTypes::StructField((name, _, _, _)) = field {
                    let ident = format_ident!("{name}");
                    return Some(quote! {
                        #ident: #ident,
                    })
                }
                None
            });

            lines.push(quote! {
                let obj = #ident {
                    #(#fields)*
                };
            });
        }
    }

    // If the type has compression support, then check the `__zstd` flag. Otherwise, use the default
    // code branch. However, even if it's a type with compression support, not all values are
    // to be compressed (thus the zstd flag). Ideally only the bigger ones.
    is_zstd
        .then(|| {
            let decompressor = format_ident!("{}_DECOMPRESSOR", ident.to_string().to_uppercase());
            quote! {
                if flags.__zstd() != 0 {
                    #decompressor.with(|decompressor| {
                        let mut decompressor = decompressor.borrow_mut();

                        let mut tmp: Vec<u8> = Vec::with_capacity(300);

                        while let Err(err) = decompressor.decompress_to_buffer(&buf[..], &mut tmp) {
                            let err = err.to_string();
                            if !err.contains("Destination buffer is too small") {
                                panic!("Failed to decompress: {}", err);
                            }
                            tmp.reserve(tmp.capacity() + 10_000);
                        }
                        let mut original_buf = buf;

                        let mut buf: &[u8] = tmp.as_slice();
                        #(#lines)*
                        (obj, original_buf)
                    })
                } else {
                    #(#lines)*
                    (obj, buf)
                }
            }
        })
        .unwrap_or_else(|| {
            quote! {
                #(#lines)*
                (obj, buf)
            }
        })
}

/// Generates code to implement the `Compact` trait method `from_compact`.
fn generate_to_compact(fields: &FieldList, ident: &Ident, is_zstd: bool) -> Vec<TokenStream2> {
    let mut lines = vec![quote! {
        let mut buffer = bytes::BytesMut::new();
    }];

    let is_enum = fields.iter().any(|field| matches!(field, FieldTypes::EnumVariant(_)));

    if is_enum {
        let enum_lines = EnumHandler::new(fields).generate_to(ident);

        lines.push(quote! {
            flags.set_variant(match self {
                #(#enum_lines)*
            });
        })
    } else {
        lines.append(&mut StructHandler::new(fields).generate_to());
    }

    // Just because a type supports compression, doesn't mean all its values are to be compressed.
    // We skip the smaller ones, and thus require a flag `__zstd` to specify if this value is
    // compressed or not.
    if is_zstd {
        lines.push(quote! {
            let mut zstd = buffer.len() > 7;
            if zstd {
                flags.set___zstd(1);
            }
        });
    }

    // Places the flag bits.
    lines.push(quote! {
        let flags = flags.into_bytes();
        total_length += flags.len() + buffer.len();
        buf.put_slice(&flags);
    });

    if is_zstd {
        let compressor = format_ident!("{}_COMPRESSOR", ident.to_string().to_uppercase());

        lines.push(quote! {
            if zstd {
                #compressor.with(|compressor| {
                    let mut compressor = compressor.borrow_mut();

                    let compressed = compressor.compress(&buffer).expect("Failed to compress.");
                    buf.put(compressed.as_slice());
                });
            } else {
                buf.put(buffer);
            }
        });
    } else {
        lines.push(quote! {
            buf.put(buffer);
        })
    }

    lines
}
