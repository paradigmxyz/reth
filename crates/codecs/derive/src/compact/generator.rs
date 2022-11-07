//! Code generator for the [`Compact`] trait.

use super::*;

/// Generates the flag fieldset struct that is going to be used to store the length of fields and
/// their potential presence.
pub fn generate_flag_struct(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let mut field_flags = vec![];
    let mut total_bits = 0;

    // Find out the adequate bit size for the length of each field, if applicable.
    for (name, ftype, is_compact) in fields {
        if *is_compact && !is_hash_type(ftype) {
            let name = format_ident!("{name}_len");
            let bitsize = get_bit_size(ftype);
            let bsize = format_ident!("B{bitsize}");
            total_bits += bitsize;

            field_flags.push(quote! {
                #name: #bsize ,
            });
        }
    }

    // Total number of bits should be divisible by 8, so we might need to pad the struct with a
    // skipped field.
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
        struct #flags {
            #(#field_flags)*
        }

        impl #flags {
            fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (#flags::from_bytes([
                    #(#readable_bytes)*
                ]), buf)
            }
        }
    }
}

/// Generates code to implement the [`Compact`] trait for a data type.
pub fn generate_from_to(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let to_compact = generate_to_compact(fields);
    let from_compact = generate_from_compact(fields, ident);

    // Build function
    quote! {
        impl Compact for #ident {
            type Encoded = Vec<u8>;

            fn to_compact(self) -> (usize, Self::Encoded) {
                let mut buffer = vec![];
                let mut flags = #flags::default();
                #(#to_compact)*
                (buffer.len(), buffer)
            }

            fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                let (flags, mut buf) = #flags::from(buf);
                #(#from_compact)*
                (obj, buf)
            }
        }
    }
}

/// Generates code to implement the [`Compact`] trait method `to_compact`.
fn generate_from_compact(fields: &FieldList, ident: &Ident) -> Vec<TokenStream2> {
    let mut lines = vec![];

    // Sets the TypeFlags with the buffer length
    for (name, ftype, is_compact) in fields {
        let (name, _, _, len) = get_field_idents(name);

        if ftype == "bytes::Bytes" {
            lines.push(quote! {
                let mut #name = bytes::Bytes::new();
                (#name, buf) = bytes::Bytes::from_compact(buf, flags.#len() as usize);
            })
        } else {
            let ident_type = format_ident!("{ftype}");
            lines.push(quote! {
                let mut #name = #ident_type::default();
            });
            if is_hash_type(ftype) {
                lines.push(quote! {
                    (#name, buf) = #ident_type::from_compact(buf, buf.len());
                })
            } else if *is_compact {
                lines.push(quote! {
                    (#name, buf) = #ident_type::from_compact(buf, flags.#len() as usize);
                });
            } else {
                todo!()
            }
        }
        // lines.push(quote! {
        //     dbg!(&#name);
        // });
    }
    let fields = fields
        .iter()
        .map(|field| {
            let ident = format_ident!("{}", field.0);
            quote! {
                #ident: #ident,
            }
        })
        .collect::<Vec<_>>();

    lines.push(quote! {
        let obj = #ident {
            #(#fields)*
        };
    });
    lines
}

/// Generates code to implement the [`Compact`] trait method `from_compact`.
fn generate_to_compact(fields: &FieldList) -> Vec<TokenStream2> {
    let mut lines = vec![];
    // Sets the TypeFlags with the buffer length
    for (name, ftype, is_compact) in fields {
        let (name, set_len_method, slice, len) = get_field_idents(name);

        if *is_compact {
            lines.push(quote! {
                let (#len, #slice) = self.#name.to_compact();
            });
        } else {
            todo!()
        }

        if !is_hash_type(ftype) {
            lines.push(quote! {
                flags.#set_len_method(dbg!(#len) as u8);
            })
        }
    }
    // Initializes buffer with the flag bits
    lines.push(quote! {
        buffer.extend_from_slice(&flags.into_bytes());
    });
    // Extends buffer with the field encoded values
    for (name, _, is_compact) in fields {
        let (_, _, slice, len) = get_field_idents(name);

        if *is_compact {
            lines.push(quote! {
                if #len > 0 {
                    buffer.extend_from_slice(#slice.as_ref())
                }
            })
        } else {
            todo!()
        }
    }
    lines
}
