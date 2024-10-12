pub(crate) mod eip1559;
pub(crate) mod eip2930;
pub(crate) mod eip4844;
pub(crate) mod eip7702;
pub(crate) mod legacy;
#[cfg(feature = "optimism")]
pub(crate) mod optimism;

#[cfg(test)]
mod tests {

    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility

    #[cfg(feature = "optimism")]
    use crate::alloy::transaction::optimism::TxDeposit;
    use crate::alloy::transaction::{
        eip1559::TxEip1559, eip2930::TxEip2930, eip4844::TxEip4844, eip7702::TxEip7702,
        legacy::TxLegacy,
    };

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(TxEip4844::bitflag_encoded_bytes(), 5);
        assert_eq!(TxLegacy::bitflag_encoded_bytes(), 3);
        assert_eq!(TxEip1559::bitflag_encoded_bytes(), 4);
        assert_eq!(TxEip2930::bitflag_encoded_bytes(), 3);
        assert_eq!(TxEip7702::bitflag_encoded_bytes(), 4);
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn test_ensure_backwards_compatibility_optimism() {
        assert_eq!(TxDeposit::bitflag_encoded_bytes(), 2);
    }
}
