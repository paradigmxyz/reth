use syn::{Field, Meta, NestedMeta};

pub(crate) fn has_attribute(field: &Field, attr_name: &str) -> bool {
    field.attrs.iter().any(|attr| {
        if attr.path.is_ident("rlp") {
            if let Ok(Meta::List(meta)) = attr.parse_meta() {
                if let Some(NestedMeta::Meta(meta)) = meta.nested.first() {
                    return meta.path().is_ident(attr_name)
                }
                return false
            } else {
            }
        }
        false
    })
}
