use super::*;

/// Generates `from_compact` code for an enum variant.
///
/// `fields_iterator` might look something like \[VariantUnit, VariantUnamedField, Field,
/// VariantUnit...\].
pub(crate) fn handle_enum_variant_from(
    variant_name: &str,
    fields_iterator: &mut std::iter::Peekable<std::iter::Enumerate<std::slice::Iter<FieldTypes>>>,
    enum_lines: &mut Vec<TokenStream2>,
    variant_index: &mut u8,
    ident: &Ident,
) {
    let variant_name = format_ident!("{variant_name}");
    if let Some((_, next_field)) = fields_iterator.peek() {
        match next_field {
            FieldTypes::EnumUnnamedField(next_ftype) => {
                // This variant is of the type `EnumVariant(UnnamedField)`
                let field_type = format_ident!("{next_ftype}");

                // Unamed type
                enum_lines.push(quote! {
                    #variant_index => {
                        let mut inner = #field_type::default();
                        (inner, buf) = #field_type::from_compact(buf, buf.len());
                        #ident::#variant_name(inner)
                    }
                });
                fields_iterator.next();
            }
            FieldTypes::EnumVariant(_) => enum_lines.push(quote! {
                #variant_index => #ident::#variant_name,
            }),
            FieldTypes::StructField(_) => unreachable!(),
        };
    } else {
        // This variant has no fields: Unit type
        enum_lines.push(quote! {
            #variant_index => #ident::#variant_name,
        });
    }
    *variant_index += 1;
}

/// Generates `to_compact` code for an enum variant.
///
/// `fields_iterator` might look something like [VariantUnit, VariantUnamedField, Field,
/// VariantUnit...].
pub(crate) fn handle_enum_variant_to(
    variant_name: &str,
    variant_index: &mut u8,
    fields_iterator: &mut std::iter::Peekable<std::slice::Iter<FieldTypes>>,
    enum_lines: &mut Vec<TokenStream2>,
    ident: &Ident,
) {
    let variant_name = format_ident!("{variant_name}");
    if let Some(next_field) = fields_iterator.peek() {
        match next_field {
            FieldTypes::EnumUnnamedField(_) => {
                // Unamed type
                enum_lines.push(quote! {
                    #ident::#variant_name(field) => {
                        field.to_compact(&mut buffer);
                        #variant_index
                    },
                });
                fields_iterator.next();
            }
            FieldTypes::EnumVariant(_) => enum_lines.push(quote! {
                #ident::#variant_name => #variant_index,
            }),
            FieldTypes::StructField(_) => unreachable!(),
        };
    } else {
        // This variant has no fields: Unit type
        enum_lines.push(quote! {
            #ident::#variant_name => #variant_index,
        });
    }
    *variant_index += 1;
}
