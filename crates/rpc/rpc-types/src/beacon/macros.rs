
/// A helper type that generates helper types for the beacon API.
macro_rules! beacon_serde_glue {
    // Named-Struct
    (
        $( #[$meta:meta] )*
        $vis:vis struct $name:ident {
            $(
                $( #[$field_meta:meta] )*
                $field_vis:vis $field_name:ident : $field_ty:ty
            ),*
        $(,)? }
    ) => {
        $( #[$meta] )*
        $vis struct Beacon$name {
            $(
                $( #[$field_meta] )*
                $field_vis $field_name : $field_ty
            ),*
        }
    }
}

pub(crate) use beacon_serde_glue;
