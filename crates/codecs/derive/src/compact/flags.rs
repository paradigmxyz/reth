use super::*;

/// Generates the flag fieldset struct that is going to be used to store the length of fields and
/// their potential presence.
pub(crate) fn generate_flag_struct(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let is_enum = fields.iter().any(|field| matches!(field, FieldTypes::EnumVariant(_)));

    let flags_ident = format_ident!("{ident}Flags");
    let mut field_flags = vec![];

    let total_bits = if is_enum {
        field_flags.push(quote! {
            variant: B8,
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
        )
    };

    if total_bits == 0 {
        return placeholder_flag_struct(&flags_ident)
    }

    let total_bytes = pad_flag_struct(total_bits, &mut field_flags);

    // Provides the number of bytes used to represent the flag struct.
    let readable_bytes = vec![
        quote! {
            buf.get_u8(),
        };
        total_bytes.into()
    ];

    // Generate the flag struct.
    quote! {
        #[bitfield]
        #[derive(Clone, Copy, Debug, Default)]
        struct #flags_ident {
            #(#field_flags)*
        }

        impl #flags_ident {
            fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (#flags_ident::from_bytes([
                    #(#readable_bytes)*
                ]), buf)
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
                    #name: #bsize ,
                });
            } else {
                let name = format_ident!("{name}");

                field_flags.push(quote! {
                    #name: bool ,
                });

                total_bits += 1;
            }
        }
    }
    total_bits
}

/// Total number of bits should be divisible by 8, so we might need to pad the struct with an unused
/// skipped field.
///
/// Returns the total number of bytes used by the flags struct.
fn pad_flag_struct(total_bits: u8, field_flags: &mut Vec<TokenStream2>) -> u8 {
    let remaining = 8 - total_bits % 8;
    let total_bytes = if remaining != 8 {
        let bsize = format_ident!("B{remaining}");
        field_flags.push(quote! {
            #[skip]
            unused: #bsize ,
        });
        (total_bits + remaining) / 8
    } else {
        total_bits / 8
    };
    total_bytes
}

/// Placeholder struct for when there are no bitfields to be added.
fn placeholder_flag_struct(flags: &Ident) -> TokenStream2 {
    quote! {
        #[derive(Debug, Default)]
        struct #flags {
        }

        impl #flags {
            fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (#flags::default(), buf)
            }
            fn into_bytes(self) -> [u8; 0] {
                []
            }
        }
    }
}
