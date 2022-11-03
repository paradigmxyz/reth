extern crate proc_macro2;
use proc_macro::{self, TokenStream};
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput};

type IsCompact = bool;
type FieldName = String;
type FieldType = String;
type FieldList = Vec<(FieldName, FieldType, IsCompact)>;

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
    return 6
}

pub fn is_hash_type(ftype: &str) -> bool {
    let known = ["H256", "H160", "Address"];
    known.contains(&ftype)
}

pub fn build_flag_struct(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let flags = syn::Ident::new(&format!("{}Flags", ident), ident.span());

    let mut field_flags = vec![];
    let mut total_bits = 0;

    for (name, ftype, is_compact) in fields {
        if *is_compact && !is_hash_type(ftype) {
            let name = syn::Ident::new(&format!("{name}_len"), ident.span());
            let bitsize = get_bit_size(ftype);
            let bsize = syn::Ident::new(&format!("B{bitsize}"), ident.span());
            total_bits += bitsize;

            field_flags.push(quote! {
                #name: #bsize ,
            });
        }
    }

    let remaining = 8 - total_bits % 8;
    let total_bytes = if remaining != 8 {
        let bsize = syn::Ident::new(&format!("B{remaining}"), ident.span());

        field_flags.push(quote! {
            #[skip]
            unused: #bsize ,
        });
        (total_bits + remaining) / 8
    } else {
        total_bits / 8
    };

    let bytes = vec![
        quote! {
            buf.get_u8(),
        };
        total_bytes.into()
    ];

    let out = quote! {
        #[bitfield]
        #[derive(Clone, Copy, Debug, Default)]
        struct #flags {
            #(#field_flags)*
        }

        impl #flags {
            fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (#flags::from_bytes([
                    #(#bytes)*
                ]), buf)
            }
        }
    };
    out.into()
}

pub fn get_field_idents(name: &str, ident: &Ident) -> (Ident, Ident, Ident, Ident) {
    let name = syn::Ident::new(&{ name }, ident.span());
    let set_len_method = syn::Ident::new(&format!("set_{name}_len"), ident.span());
    let slice = syn::Ident::new(&format!("{name}_slice"), ident.span());
    let len = syn::Ident::new(&format!("{name}_len"), ident.span());
    (name, set_len_method, slice, len)
}

pub fn build_encode(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let flags = syn::Ident::new(&format!("{}Flags", ident), ident.span());

    let to_compact = build_to_compact(fields, ident);
    let from_compact = build_from_compact(fields, ident);

    // Build function
    let mut out = quote! {
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
    };
    out.into()
}

fn build_from_compact(fields: &FieldList, ident: &Ident) -> Vec<TokenStream2> {
    let mut lines = vec![];

    // Sets the TypeFlags with the buffer length
    for (name, ftype, is_compact) in fields {
        let (name, set_len_method, slice, len) = get_field_idents(name, ident);

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
            let ident = syn::Ident::new(&format!("{}", field.0), ident.span());
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

fn build_to_compact(fields: &FieldList, ident: &Ident) -> Vec<TokenStream2> {
    let mut lines = vec![];
    // Sets the TypeFlags with the buffer length
    for (name, ftype, is_compact) in fields {
        let (name, set_len_method, slice, len) = get_field_idents(name, ident);

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
    for (name, ftype, is_compact) in fields {
        let (_, _, slice, len) = get_field_idents(name, ident);

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

pub fn derive(input: TokenStream) -> TokenStream {
    let DeriveInput { ident, data, .. } = parse_macro_input!(input);

    let fields = get_fields(&data);

    let mut output = quote! {};
    output.extend(build_flag_struct(&ident, &fields));
    output.extend(build_encode(&ident, &fields));
    output.into()
}
