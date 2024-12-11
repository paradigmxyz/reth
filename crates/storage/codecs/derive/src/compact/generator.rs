//! Code generator for the `Compact` trait.

use super::*;
use crate::ZstdConfig;
use convert_case::{Case, Casing};
use syn::{Attribute, LitStr};

/// Generates code to implement the `Compact` trait for a data type.
pub fn generate_from_to(
    ident: &Ident,
    attrs: &[Attribute],
    has_lifetime: bool,
    fields: &FieldList,
    zstd: Option<ZstdConfig>,
) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let reth_codecs = parse_reth_codecs_path(attrs).unwrap();

    let to_compact = generate_to_compact(fields, ident, zstd.clone(), &reth_codecs);
    let from_compact = generate_from_compact(fields, ident, zstd);

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
           impl<#lifetime> #reth_codecs::Compact for #ident<#lifetime>
        }
    } else {
        quote! {
           impl #reth_codecs::Compact for #ident
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
                use #reth_codecs::Compact;
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
            fn to_compact<B>(&self, buf: &mut B) -> usize where B: #reth_codecs::__private::bytes::BufMut + AsMut<[u8]> {
                let mut flags = #flags::default();
                let mut total_length = 0;
                #(#to_compact)*
                total_length
            }

            fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                #fn_from_compact
            }
        }
    }
}

/// Generates code to implement the `Compact` trait method `to_compact`.
fn generate_from_compact(
    fields: &FieldList,
    ident: &Ident,
    zstd: Option<ZstdConfig>,
) -> TokenStream2 {
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
    if let Some(zstd) = zstd {
        let decompressor = zstd.decompressor;
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
    } else {
        quote! {
            #(#lines)*
            (obj, buf)
        }
    }
}

/// Generates code to implement the `Compact` trait method `from_compact`.
fn generate_to_compact(
    fields: &FieldList,
    ident: &Ident,
    zstd: Option<ZstdConfig>,
    reth_codecs: &syn::Path,
) -> Vec<TokenStream2> {
    let mut lines = vec![quote! {
        let mut buffer = #reth_codecs::__private::bytes::BytesMut::new();
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
    // We skip the smaller ones, and thus require a flag` __zstd` to specify if this value is
    // compressed or not.
    if zstd.is_some() {
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

    if let Some(zstd) = zstd {
        let compressor = zstd.compressor;
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

/// Function to extract the crate path from `reth_codecs(crate = "...")` attribute.
pub(crate) fn parse_reth_codecs_path(attrs: &[Attribute]) -> syn::Result<syn::Path> {
    // let default_crate_path: syn::Path = syn::parse_str("reth-codecs").unwrap();
    let mut reth_codecs_path: syn::Path = syn::parse_quote!(reth_codecs);
    for attr in attrs {
        if attr.path().is_ident("reth_codecs") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    reth_codecs_path = syn::parse_str(&lit.value())?;
                    Ok(())
                } else {
                    Err(meta.error("unsupported attribute"))
                }
            })?;
        }
    }

    Ok(reth_codecs_path)
}
