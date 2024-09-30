//! Code generator for the `Compact` trait.

use super::*;
use convert_case::{Case, Casing};

/// Generates code to implement the `Compact` trait for a data type.
pub fn generate_from_to(
    ident: &Ident,
    has_lifetime: bool,
    fields: &FieldList,
    is_zstd: bool,
) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let to_compact_inner = generate_to_compact(fields, ident, is_zstd);
    let from_compact = generate_from_compact(fields, ident, is_zstd);

    let to_compact = if is_zstd {
        let compressor = format_ident!("{}_COMPRESSOR", ident.to_string().to_uppercase());
        quote! {
            // Encode to a local buffer first.
            let mut tmp_buffer = Vec::with_capacity(64);
            let flags = self.to_compact_inner(&mut tmp_buffer).into_bytes();

            buffer.put_slice(&flags);
            #compressor.with(|compressor| {
                let mut compressor = compressor.borrow_mut();
                // TODO: Write directly to `buffer` with `compress_to_buffer`.
                let compressed = compressor.compress(&tmp_buffer).expect("Failed to compress.");
                buffer.put_slice(compressed.as_slice());
            });
        }
    } else {
        quote! {
            // Encode a placeholder for the flags which we fill later.
            if Self::BITFLAG_ENCODED_BYTES > 0 {
                buffer.put_slice(&[0; Self::BITFLAG_ENCODED_BYTES]);
            }

            let flags = self.to_compact_inner(buffer).into_bytes();
            debug_assert_eq!(Self::BITFLAG_ENCODED_BYTES, flags.len());

            if Self::BITFLAG_ENCODED_BYTES > 0 {
                buffer.as_mut()[start_len..start_len + flags.len()].copy_from_slice(&flags);
            }
        }
    };

    let snake_case_ident = ident.to_string().to_case(Case::Snake);

    let fuzz = format_ident!("fuzz_test_{snake_case_ident}");
    let test = format_ident!("fuzz_{snake_case_ident}");

    let lifetime = if has_lifetime {
        quote! { 'a }
    } else {
        quote! {}
    };

    let impl_compact = if has_lifetime {
        quote! {
           impl<#lifetime> Compact for #ident<#lifetime>
        }
    } else {
        quote! {
           impl Compact for #ident
        }
    };
    let normal_impl = if has_lifetime {
        quote! {
            impl<#lifetime> #ident<#lifetime>
        }
    } else {
        quote! {
            impl #ident
        }
    };

    let fn_from_compact = if has_lifetime {
        quote! { unimplemented!("from_compact not supported with ref structs") }
    } else {
        quote! {
            let (flags, mut buf) = #flags::from(buf);
            #from_compact
        }
    };

    let fuzz_tests = if has_lifetime {
        quote! {}
    } else {
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
            #[allow(missing_docs)]
            pub fn #test() {
                #fuzz(#ident::default())
            }
        }
    };

    // Build function
    quote! {
        #fuzz_tests

        #impl_compact {
            fn to_compact<B>(&self, buffer: &mut B) -> usize where B: bytes::BufMut + AsMut<[u8]> {
                let start_len = buffer.as_mut().len();
                #to_compact
                buffer.as_mut().len() - start_len
            }

            fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                #fn_from_compact
            }
        }

        #normal_impl {
            fn to_compact_inner<B>(&self, buffer: &mut B) -> #flags where B: bytes::BufMut + AsMut<[u8]> {
                let mut flags = #flags::default();
                let start_len = buffer.as_mut().len();
                #(#to_compact_inner)*
                flags
            }
        }
    }
}

/// Generates code to implement the `Compact` trait method `to_compact`.
fn generate_from_compact(fields: &FieldList, ident: &Ident, is_zstd: bool) -> TokenStream2 {
    let mut lines = vec![];
    let mut known_types =
        vec!["B256", "Address", "Bloom", "Vec", "TxHash", "BlockHash", "FixedBytes"];

    // Only types without `Bytes` should be added here. It's currently manually added, since
    // it's hard to figure out with derive_macro which types have Bytes fields.
    //
    // This removes the requirement of the field to be placed last in the struct.
    known_types.extend_from_slice(&["TxKind", "AccessList", "Signature", "CheckpointBlockRange"]);

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
                        let decompressor = &mut decompressor.borrow_mut();
                        let decompressed = decompressor.decompress(buf);
                        let mut original_buf = buf;

                        let mut buf: &[u8] = decompressed;
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
    let mut lines = vec![];

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
            if (buffer.as_mut().len() - start_len) > 7 {
                flags.set___zstd(1);
            }
        });
    }

    lines
}
