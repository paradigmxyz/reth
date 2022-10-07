use crate::U256;

/// Signature TODO
/// r, s: Values corresponding to the signature of the
/// transaction and used to determine the sender of
/// the transaction; formally Tr and Ts. This is expanded in Appendix F of yellow paper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The R field of the signature; the point on the curve.
    pub r: U256,
    /// The S field of the signature; the point on the curve.
    pub s: U256,
    /// yParity: Signature Y parity; forma Ty
    pub y_parity: u8,
}
