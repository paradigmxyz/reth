use super::*;

#[derive(Debug)]
pub struct EnumHandler<'a> {
    current_variant_index: u8,
    fields_iterator: std::iter::Peekable<std::slice::Iter<'a, FieldTypes>>,
    enum_lines: Vec<TokenStream2>,
}

impl<'a> EnumHandler<'a> {
    pub fn new(fields: &'a FieldList) -> Self {
        EnumHandler {
            current_variant_index: 0u8,
            enum_lines: vec![],
            fields_iterator: fields.iter().peekable(),
        }
    }

    pub fn next_field(&mut self) -> Option<&'a FieldTypes> {
        self.fields_iterator.next()
    }

    pub fn generate_to(mut self, ident: &Ident) -> Vec<TokenStream2> {
        while let Some(field) = self.next_field() {
            match field {
                //  The following method will advance the
                // `fields_iterator` by itself and stop right before the next variant.
                FieldTypes::EnumVariant(name) => self.to(name, ident),
                FieldTypes::EnumUnnamedField(_) => unreachable!(),
                FieldTypes::StructField(_) => unreachable!(),
            }
        }
        self.enum_lines
    }

    pub fn generate_from(mut self, ident: &Ident) -> Vec<TokenStream2> {
        while let Some(field) = self.next_field() {
            match field {
                //  The following method will advance the
                // `fields_iterator` by itself and stop right before the next variant.
                FieldTypes::EnumVariant(name) => self.from(name, ident),
                FieldTypes::EnumUnnamedField(_) => unreachable!(),
                FieldTypes::StructField(_) => unreachable!(),
            }
        }
        self.enum_lines
    }

    /// Generates `from_compact` code for an enum variant.
    ///
    /// `fields_iterator` might look something like \[VariantUnit, VariantUnamedField, Field,
    /// VariantUnit...\].
    pub fn from(&mut self, variant_name: &str, ident: &Ident) {
        let variant_name = format_ident!("{variant_name}");
        let current_variant_index = self.current_variant_index;

        if let Some(next_field) = self.fields_iterator.peek() {
            match next_field {
                FieldTypes::EnumUnnamedField(next_ftype) => {
                    // This variant is of the type `EnumVariant(UnnamedField)`
                    let field_type = format_ident!("{next_ftype}");

                    // Unamed type
                    self.enum_lines.push(quote! {
                        #current_variant_index => {
                            let mut inner = #field_type::default();
                            (inner, buf) = #field_type::from_compact(buf, buf.len());
                            #ident::#variant_name(inner)
                        }
                    });
                    self.fields_iterator.next();
                }
                FieldTypes::EnumVariant(_) => self.enum_lines.push(quote! {
                    #current_variant_index => #ident::#variant_name,
                }),
                FieldTypes::StructField(_) => unreachable!(),
            };
        } else {
            // This variant has no fields: Unit type
            self.enum_lines.push(quote! {
                #current_variant_index => #ident::#variant_name,
            });
        }
        self.current_variant_index += 1;
    }

    /// Generates `to_compact` code for an enum variant.
    ///
    /// `fields_iterator` might look something like [VariantUnit, VariantUnamedField, Field,
    /// VariantUnit...].
    pub fn to(&mut self, variant_name: &str, ident: &Ident) {
        let variant_name = format_ident!("{variant_name}");
        let current_variant_index = self.current_variant_index;

        if let Some(next_field) = self.fields_iterator.peek() {
            match next_field {
                FieldTypes::EnumUnnamedField(_) => {
                    // Unamed type
                    self.enum_lines.push(quote! {
                        #ident::#variant_name(field) => {
                            field.to_compact(&mut buffer);
                            #current_variant_index
                        },
                    });
                    self.fields_iterator.next();
                }
                FieldTypes::EnumVariant(_) => self.enum_lines.push(quote! {
                    #ident::#variant_name => #current_variant_index,
                }),
                FieldTypes::StructField(_) => unreachable!(),
            };
        } else {
            // This variant has no fields: Unit type
            self.enum_lines.push(quote! {
                #ident::#variant_name => #current_variant_index,
            });
        }
        self.current_variant_index += 1;
    }
}
