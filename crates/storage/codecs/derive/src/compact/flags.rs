use super::*;

/// Generates the flag fieldset struct that is going to be used to store the length of fields and
/// their potential presence.
pub(crate) fn generate_flag_struct(
    ident: &Ident,
    has_lifetime: bool,
    fields: &FieldList,
    is_zstd: bool,
) -> TokenStream2 {
    let is_enum = fields.iter().any(|field| matches!(field, FieldTypes::EnumVariant(_)));

    let flags_ident = format_ident!("{ident}Flags");
    let mod_flags_ident = format_ident!("{ident}_flags");

    let mut field_flags = vec![];

    let total_bits = if is_enum {
        field_flags.push(quote! {
            pub variant: B8,
        });
        8
    } else {
        build_struct_field_flags(
            fields
                .iter()
                .filter_map(|f| {
                    if let FieldTypes::StructField(f) = f {
                        return Some(f)
                    }
                    None
                })
                .collect::<Vec<_>>(),
            &mut field_flags,
            is_zstd,
        )
    };

    if total_bits == 0 {
        return placeholder_flag_struct(ident, &flags_ident)
    }

    let (total_bytes, unused_bits) = pad_flag_struct(total_bits, &mut field_flags);

    // Provides the number of bytes used to represent the flag struct.
    let readable_bytes = vec![
        quote! {
            buf.get_u8(),
        };
        total_bytes.into()
    ];

    let docs =
        format!("Fieldset that facilitates compacting the parent type. Used bytes: {total_bytes} | Unused bits: {unused_bits}");
    let bitflag_encoded_bytes = format!("Used bytes by [`{flags_ident}`]");
    let impl_bitflag_encoded_bytes = if has_lifetime {
        quote! {
            impl<'a> #ident<'a> {
                #[doc = #bitflag_encoded_bytes]
                pub const fn bitflag_encoded_bytes() -> usize {
                    #total_bytes as usize
                }
           }
        }
    } else {
        quote! {
            impl #ident {
                #[doc = #bitflag_encoded_bytes]
                pub const fn bitflag_encoded_bytes() -> usize {
                    #total_bytes as usize
                }
           }
        }
    };

    // Generate the flag struct.
    quote! {
        #impl_bitflag_encoded_bytes
        pub use #mod_flags_ident::#flags_ident;
        #[allow(non_snake_case)]
        mod #mod_flags_ident {
            use bytes::Buf;
            use modular_bitfield::prelude::*;

            #[doc = #docs]
            #[bitfield]
            #[derive(Clone, Copy, Debug, Default)]
            pub struct #flags_ident {
                #(#field_flags)*
            }

            impl #flags_ident {
                /// Deserializes this fieldset and returns it, alongside the original slice in an advanced position.
                pub fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                    (#flags_ident::from_bytes([
                        #(#readable_bytes)*
                    ]), buf)
                }
            }
        }
    }
}

/// Builds the flag struct for the user struct fields.
///
/// Returns the total number of bits necessary.
fn build_struct_field_flags(
    fields: Vec<&StructFieldDescriptor>,
    field_flags: &mut Vec<TokenStream2>,
    is_zstd: bool,
) -> u8 {
    let mut total_bits = 0;

    // Find out the adequate bit size for the length of each field, if applicable.
    for (name, ftype, is_compact, _) in fields {
        // This happens when dealing with a wrapper struct eg. Struct(pub U256).
        let name = if name.is_empty() { "placeholder" } else { name };

        if *is_compact {
            if is_flag_type(ftype) {
                let name = format_ident!("{name}_len");
                let bitsize = get_bit_size(ftype);
                let bsize = format_ident!("B{bitsize}");
                total_bits += bitsize;

                field_flags.push(quote! {
                    pub #name: #bsize ,
                });
            } else {
                let name = format_ident!("{name}");

                field_flags.push(quote! {
                    pub #name: bool ,
                });

                total_bits += 1;
            }
        }
    }

    if is_zstd {
        field_flags.push(quote! {
            pub __zstd: B1,
        });

        total_bits += 1;
    }

    total_bits
}

/// Total number of bits should be divisible by 8, so we might need to pad the struct with an unused
/// skipped field.
///
/// Returns the total number of bytes used by the flags struct and how many unused bits.
fn pad_flag_struct(total_bits: u8, field_flags: &mut Vec<TokenStream2>) -> (u8, u8) {
    let remaining = 8 - total_bits % 8;
    if remaining != 8 {
        let bsize = format_ident!("B{remaining}");
        field_flags.push(quote! {
            #[skip]
            unused: #bsize ,
        });
        ((total_bits + remaining) / 8, remaining)
    } else {
        (total_bits / 8, 0)
    }
}

/// Placeholder struct for when there are no bitfields to be added.
fn placeholder_flag_struct(ident: &Ident, flags: &Ident) -> TokenStream2 {
    let bitflag_encoded_bytes = format!("Used bytes by [`{flags}`]");
    let bitflag_unused_bits = format!("Unused bits for new fields by [`{flags}`]");
    quote! {
        impl #ident {
            #[doc = #bitflag_encoded_bytes]
            pub const fn bitflag_encoded_bytes() -> usize {
                0
            }

            #[doc = #bitflag_unused_bits]
            pub const fn bitflag_unused_bits() -> usize {
                0
            }
        }

        /// Placeholder struct for when there is no need for a fieldset. Doesn't actually write or read any data.
        #[derive(Debug, Default)]
        pub struct #flags {
        }

        impl #flags {
            /// Placeholder: does not read any value.
            pub fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (#flags::default(), buf)
            }
            /// Placeholder: returns an empty array.
            pub fn into_bytes(self) -> [u8; 0] {
                []
            }
        }
    }
}
