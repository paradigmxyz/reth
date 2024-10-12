use syn::{Attribute, DeriveInput, Field};

pub(crate) trait WithAttrs {
    fn attrs(&self) -> &[Attribute];
}

impl WithAttrs for DeriveInput {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
}

impl WithAttrs for Field {
    fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }
}
