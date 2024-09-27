/// Failed to decode a key
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display)]
#[display("failed to decode a key from a table")]
pub struct DecodeError;
