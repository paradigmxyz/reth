use crate::H256;

/// Signature TODO
/// r, s: Values corresponding to the signature of the
/// transaction and used to determine the sender of
/// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub r: H256,
    pub s: H256,
    pub y_parity: u8,
}
