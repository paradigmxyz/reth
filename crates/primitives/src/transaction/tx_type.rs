/// Transaction Type
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TxType {
    /// Legacy transaction pre EIP-2929
    Legacy = 0,
    /// AccessList transaction
    EIP2930 = 1,
    /// Transaction with Priority fee
    EIP1559 = 2,
}
