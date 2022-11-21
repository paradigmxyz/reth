use super::*;

/// Generates `from_compact` code for a struct field.
pub(crate) fn handle_struct_field_from(
    name: &str,
    known_types: [&str; 5],
    field_num: usize,
    fields: &Vec<FieldTypes>,
    ftype: &str,
    lines: &mut Vec<TokenStream2>,
    is_compact: &bool,
) {
    let (name, _, len) = get_field_idents(name);
    assert!(
        known_types.contains(&ftype) || is_flag_type(ftype) || field_num == fields.len() - 1,
        "{ftype} field should be placed as the last one since it's not known. "
    );
    if ftype == "bytes::Bytes" {
        lines.push(quote! {
            let mut #name = bytes::Bytes::new();
            (#name, buf) = bytes::Bytes::from_compact(buf, buf.len() as usize);
        })
    } else {
        let ident_type = format_ident!("{ftype}");
        lines.push(quote! {
            let mut #name = #ident_type::default();
        });
        if !is_flag_type(ftype) {
            // It's a type that handles its own length requirements. (h256, Custom, ...)
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
}

/// Generates `from_compact` code for a struct field.
pub(crate) fn handle_struct_field_to(
    is_compact: &bool,
    ftype: &str,
    lines: &mut Vec<TokenStream2>,
    name: Ident,
    len: Ident,
    set_len_method: Ident,
) {
    // H256 with #[maybe_zero] attribute for example
    if *is_compact && !is_flag_type(ftype) {
        let itype = format_ident!("{ftype}");
        let set_bool_method = format_ident!("set_{name}");
        lines.push(quote! {
            if self.#name != #itype::zero() {
                flags.#set_bool_method(true);
                self.#name.to_compact(&mut buffer);
            };
        });
    } else {
        lines.push(quote! {
            let #len = self.#name.to_compact(&mut buffer);
        });
    }
    if is_flag_type(ftype) {
        lines.push(quote! {
            flags.#set_len_method(#len as u8);
        })
    }
}
