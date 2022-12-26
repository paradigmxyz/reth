use syn::{Error, Result, Type, TypePath};

#[derive(Debug)]
pub(crate) struct LooseTypePath(TypePath);

impl LooseTypePath {
    pub(crate) fn parse_from_ty(ty: Type) -> Result<Self> {
        match ty {
            Type::Path(path) => Ok(Self(path)),
            _ => Err(Error::new_spanned(ty, "Type is not a path.")),
        }
    }
}

impl PartialEq for LooseTypePath {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.path.segments.last(), other.0.path.segments.last()) {
            (Some(seg1), Some(seg2)) => seg1.eq(seg2),
            _ => false,
        }
    }
}
