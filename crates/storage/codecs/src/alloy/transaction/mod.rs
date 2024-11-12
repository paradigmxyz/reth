//! Compact implementation for transaction types

cond_mod!(
    eip1559,
    eip2930,
    eip4844,
    eip7702,
    legacy
);


#[cfg(all(feature = "test-utils", feature = "optimism"))]
pub mod optimism;
#[cfg(all(not(feature = "test-utils"), feature = "optimism"))]
mod optimism;

#[cfg(test)]
mod tests {

    // each value in the database has an extra field named flags that encodes metadata about other
    // fields in the value, e.g. offset and length.
    //
    // this check is to ensure we do not inadvertently add too many fields to a struct which would
    // expand the flags field and break backwards compatibility

    use alloy_primitives::hex;
    use crate::{
        alloy::{header::Header, transaction::{
            eip1559::TxEip1559, eip2930::TxEip2930, eip4844::TxEip4844, eip7702::TxEip7702,
            legacy::TxLegacy,
        }},
        test_utils::test_decode,
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
        assert_eq!(crate::alloy::transaction::optimism::TxDeposit::bitflag_encoded_bytes(), 2);
    }

    #[test]
    fn test_decode_header() {
        test_decode::<Header>(&hex!(
            "01000000fbbb564baeafd064b979c2ac032df5cd987098066a8c6969514dfb8ecfbf043e667fa19efcc00d1dd197c309a3cc42dec820cd627af8f7f38f3274f842406891b22624431d0ea858422db8415b1181f8d19befbd21287debaf98a94e84b3ec20be846f35abfbf743ee3eda4fdda6a6f9124d295da97e26eaa1cedd09936f0a3c560b6bc10316dba5e82abd21afcf519a985feb09a6ce7fba2e8163b10f06c99828b8049c29b993d88d1d112dca60a03ebd8ebc6d69a7e1f301ca6d67c21fe0949d67bca251edf36c96a2cf7c84d98fc60a53988ac95820f434eb35280d98c8ba4d7484e7ee8fefd63591ad4c937ccaaea23871d05c77bac754c5759b34cf9b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn test_decode_eip1559() {
        test_decode::<TxEip1559>(&hex!(
            "88086110b81b05bc5bb59ec3e4cd44e895a9dcb2656d5003e2f64ecb2e15443898cc1cc19af19ca96fc2b4eafc4abc26e4bbd70a3ddb10b7530b65eea128f4095c97164f712c04239902c1b08acf3949d4687123cdd72d5c73df113d2dc6ed7e519f410ace5553ca805975240a208b57013532de78c5cb407423ea11921ab11b13e93ef35d4d01c9a23166c4d627987545fe4675528d0ab111b0a1dc83fba0a4e1cd5c826a94db3f"
        ));
    }

    #[test]
    fn test_decode_eip2930() {
        test_decode::<TxEip2930>(&hex!(
            "7810833fce14e3e2921e94fd3727eb71e91551d2c1e029697a654bfab510f3963aa57074015e152065d1c807f8830079fb0aeadc251d248eaec7147e78580ed638c4e667827775e24270edd5aad475776533ece65373afa71722bfeba3c900"
        ));
    }

    #[test]
    fn test_decode_eip4844() {
        test_decode::<TxEip4844>(&hex!(
            "88086110025c359180ea680b5007c856f9e1ad4d1be7a5019feb42133f4fc4bdf74da1b457ab787462385a28a1bf8edb401adabf3ff21ac18f695e30180348ea67246fc4dc25e88add12b7c317651a0ce08946d98dbbe5b38883aa758a0f247e23b0fe3ac1bcc43d7212c984d6ccc770d70135890c9a07d715cacb9032c90d539d0b3d209a8d600178bcfb416fd489e5d5dd56d9cfc6addae810ae70bdaee65672b871dc2b3f35ec00dbaa0d872f78cb58b3199984c608c8ba"
        ));
    }

    #[test]
    fn test_decode_eip7702() {
        test_decode::<TxEip7702>(&hex!(
            "8808210881415c034feba383d7a6efd3f2601309b33a6d682ad47168cac0f7a5c5136a33370e5e7ca7f570d5530d7a0d18bf5eac33583fdc27b6580f61e8cbd34d6de596f925c1f353188feb2c1e9e20de82a80b57f0be425d8c5896280d4f5f66cdcfba256d0c9ac8abd833859a62ec019501b4585fa176f048de4f88b93bdefecfcaf4d8f0dd04767bc683a4569c893632e44ba9d53f90d758125c9b24c0192a649166520cd5eecbc110b53eda400cf184b8ef9932c81d0deb2ea27dfa863392a87bfd53af3ec67379f20992501e76e387cbe3933861beead1b49649383cf8b2a2d5c6d04b7edc376981ed9b12cf7199fe7fabf5198659e001bed40922969b82a6cd000000000000"
        ));
    }

    #[test]
    fn test_decode_legacy() {
        test_decode::<TxLegacy>(&hex!(
            "112210080a8ba06a8d108540bb3140e9f71a0812c46226f9ea77ae880d98d19fe27e5911801175c3b32620b2e887af0296af343526e439b775ee3b1c06750058e9e5fc4cd5965c3010f86184"
        ));
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn test_decode_deposit() {
        test_decode::<op_alloy_consensus::TxDeposit>(&hex!(
            "8108ac8f15983d59b6ae4911a00ff7bfcd2e53d2950926f8c82c12afad02861c46fcb293e776204052725e1c08ff2e9ff602ca916357601fa972a14094891fe3598b718758f22c46f163c18bcaa6296ce87e5267ef3fd932112842fbbf79011548cdf067d93ce6098dfc0aaf5a94531e439f30d6dfd0c6"
        )); 
    }
}
