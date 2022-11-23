//! Code generator for the [`Compact`] trait.

use super::*;

/// Generates code to implement the [`Compact`] trait for a data type.
pub fn generate_from_to(ident: &Ident, fields: &FieldList) -> TokenStream2 {
    let flags = format_ident!("{ident}Flags");

    let to_compact = generate_to_compact(fields, ident);
    let from_compact = generate_from_compact(fields, ident);

    let fuzz = format_ident!("fuzz_test_{ident}");
    let test = format_ident!("fuzz_{ident}");

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
            fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
                let mut flags = #flags::default();
                let mut total_len = 0;
                #(#to_compact)*
                total_len
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
    let known_types = ["H256", "H160", "Address", "Bloom", "Vec"];

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
        lines.append(&mut StructHandler::new(fields).generate_from(known_types.as_slice()));

        let fields = fields.iter().filter_map(|field| {
            if let FieldTypes::StructField((name, _, _)) = field {
                let ident = format_ident!("{name}");
                return Some(quote! {
                    #ident: #ident,
                })
            }
            None
        });

        // Builds the object instantiation.
        lines.push(quote! {
            let obj = #ident {
                #(#fields)*
            };
        });
    }

    lines
}

/// Generates code to implement the [`Compact`] trait method `from_compact`.
fn generate_to_compact(fields: &FieldList, ident: &Ident) -> Vec<TokenStream2> {
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

    // Places the flag bits.
    lines.push(quote! {
        let flags = flags.into_bytes();
        total_len += flags.len() + buffer.len();
        buf.put_slice(&flags);
        buf.put(buffer);
    });

    lines
}
