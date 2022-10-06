use crate::H256;

/// Signature still TODO
#[derive(Debug, Clone)]
pub struct Signature {
    pub r: H256,
    pub s: H256,
    pub y_parity: u8,
}
