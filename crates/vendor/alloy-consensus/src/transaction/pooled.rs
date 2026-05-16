//! Defines the exact transaction variant that is allowed to be propagated over the eth p2p
//! protocol.

use super::EthereumTxEnvelope;
use crate::{error::ValueError, Signed, TxEip4844, TxEip4844Variant, TxEip4844WithSidecar};
use alloy_eips::eip7594::Encodable7594;

/// All possible transactions that can be included in a response to `GetPooledTransactions`.
/// A response to `GetPooledTransactions`. This can include either a blob transaction, or a
/// non-4844 signed transaction.
///
/// The difference between this and the [`EthereumTxEnvelope<TxEip4844Variant<T>>`] is that this
/// type always requires the [`TxEip4844WithSidecar`] variant, because EIP-4844 transaction can only
/// be propagated with the sidecar over p2p.
pub type PooledTransaction = EthereumTxEnvelope<TxEip4844WithSidecar>;

impl<T: Encodable7594> EthereumTxEnvelope<TxEip4844WithSidecar<T>> {
    /// Converts the transaction into [`EthereumTxEnvelope<TxEip4844Variant<T>>`].
    pub fn into_envelope(self) -> EthereumTxEnvelope<TxEip4844Variant<T>> {
        match self {
            Self::Legacy(tx) => tx.into(),
            Self::Eip2930(tx) => tx.into(),
            Self::Eip1559(tx) => tx.into(),
            Self::Eip7702(tx) => tx.into(),
            Self::Eip4844(tx) => tx.into(),
        }
    }
}

impl<T: Encodable7594> TryFrom<Signed<TxEip4844Variant<T>>>
    for EthereumTxEnvelope<TxEip4844WithSidecar<T>>
{
    type Error = ValueError<Signed<TxEip4844Variant<T>>>;

    fn try_from(value: Signed<TxEip4844Variant<T>>) -> Result<Self, Self::Error> {
        let (value, signature, hash) = value.into_parts();
        match value {
            tx @ TxEip4844Variant::TxEip4844(_) => Err(ValueError::new_static(
                Signed::new_unchecked(tx, signature, hash),
                "pooled transaction requires 4844 sidecar",
            )),
            TxEip4844Variant::TxEip4844WithSidecar(tx) => {
                Ok(Signed::new_unchecked(tx, signature, hash).into())
            }
        }
    }
}

impl<T: Encodable7594> TryFrom<EthereumTxEnvelope<TxEip4844Variant<T>>>
    for EthereumTxEnvelope<TxEip4844WithSidecar<T>>
{
    type Error = ValueError<EthereumTxEnvelope<TxEip4844Variant<T>>>;

    fn try_from(value: EthereumTxEnvelope<TxEip4844Variant<T>>) -> Result<Self, Self::Error> {
        value.try_into_pooled()
    }
}

impl<T: Encodable7594> TryFrom<EthereumTxEnvelope<TxEip4844>>
    for EthereumTxEnvelope<TxEip4844WithSidecar<T>>
{
    type Error = ValueError<EthereumTxEnvelope<TxEip4844>>;

    fn try_from(value: EthereumTxEnvelope<TxEip4844>) -> Result<Self, Self::Error> {
        value.try_into_pooled()
    }
}

impl<T: Encodable7594> From<EthereumTxEnvelope<TxEip4844WithSidecar<T>>>
    for EthereumTxEnvelope<TxEip4844Variant<T>>
{
    fn from(tx: EthereumTxEnvelope<TxEip4844WithSidecar<T>>) -> Self {
        tx.into_envelope()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transaction;
    use alloy_eips::{Decodable2718, Encodable2718};
    use alloy_primitives::{address, hex, Bytes};
    use alloy_rlp::Decodable;
    use std::path::PathBuf;

    #[test]
    fn invalid_legacy_pooled_decoding_input_too_short() {
        let input_too_short = [
            // this should fail because the payload length is longer than expected
            &hex!("d90b0280808bc5cd028083c5cdfd9e407c56565656")[..],
            // these should fail decoding
            //
            // The `c1` at the beginning is a list header, and the rest is a valid legacy
            // transaction, BUT the payload length of the list header is 1, and the payload is
            // obviously longer than one byte.
            &hex!("c10b02808083c5cd028883c5cdfd9e407c56565656"),
            &hex!("c10b0280808bc5cd028083c5cdfd9e407c56565656"),
            // this one is 19 bytes, and the buf is long enough, but the transaction will not
            // consume that many bytes.
            &hex!("d40b02808083c5cdeb8783c5acfd9e407c5656565656"),
            &hex!("d30102808083c5cd02887dc5cdfd9e64fd9e407c56"),
        ];

        for hex_data in &input_too_short {
            let input_rlp = &mut &hex_data[..];
            let res = PooledTransaction::decode(input_rlp);

            assert!(
                res.is_err(),
                "expected err after decoding rlp input: {:x?}",
                Bytes::copy_from_slice(hex_data)
            );

            // this is a legacy tx so we can attempt the same test with decode_enveloped
            let input_rlp = &mut &hex_data[..];
            let res = PooledTransaction::decode_2718(input_rlp);

            assert!(
                res.is_err(),
                "expected err after decoding enveloped rlp input: {:x?}",
                Bytes::copy_from_slice(hex_data)
            );
        }
    }

    // <https://holesky.etherscan.io/tx/0x7f60faf8a410a80d95f7ffda301d5ab983545913d3d789615df3346579f6c849>
    #[test]
    fn decode_eip1559_enveloped() {
        let data = hex!("02f903d382426882ba09832dc6c0848674742682ed9694714b6a4ea9b94a8a7d9fd362ed72630688c8898c80b90364492d24749189822d8512430d3f3ff7a2ede675ac08265c08e2c56ff6fdaa66dae1cdbe4a5d1d7809f3e99272d067364e597542ac0c369d69e22a6399c3e9bee5da4b07e3f3fdc34c32c3d88aa2268785f3e3f8086df0934b10ef92cfffc2e7f3d90f5e83302e31382e302d64657600000000000000000000000000000000000000000000569e75fc77c1a856f6daaf9e69d8a9566ca34aa47f9133711ce065a571af0cfd000000000000000000000000e1e210594771824dad216568b91c9cb4ceed361c00000000000000000000000000000000000000000000000000000000000546e00000000000000000000000000000000000000000000000000000000000e4e1c00000000000000000000000000000000000000000000000000000000065d6750c00000000000000000000000000000000000000000000000000000000000f288000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002cf600000000000000000000000000000000000000000000000000000000000000640000000000000000000000000000000000000000000000000000000000000000f1628e56fa6d8c50e5b984a58c0df14de31c7b857ce7ba499945b99252976a93d06dcda6776fc42167fbe71cb59f978f5ef5b12577a90b132d14d9c6efa528076f0161d7bf03643cfc5490ec5084f4a041db7f06c50bd97efa08907ba79ddcac8b890f24d12d8db31abbaaf18985d54f400449ee0559a4452afe53de5853ce090000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000028000000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000064ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00000000000000000000000000000000000000000000000000000000c080a01428023fc54a27544abc421d5d017b9a7c5936ad501cbdecd0d9d12d04c1a033a0753104bbf1c87634d6ff3f0ffa0982710612306003eb022363b57994bdef445a"
);

        let res = PooledTransaction::decode_2718(&mut &data[..]).unwrap();
        assert_eq!(res.to(), Some(address!("714b6a4ea9b94a8a7d9fd362ed72630688c8898c")));
    }

    #[test]
    fn legacy_valid_pooled_decoding() {
        // d3 <- payload length, d3 - c0 = 0x13 = 19
        // 0b <- nonce
        // 02 <- gas_price
        // 80 <- gas_limit
        // 80 <- to (Create)
        // 83 c5cdeb <- value
        // 87 83c5acfd9e407c <- input
        // 56 <- v (eip155, so modified with a chain id)
        // 56 <- r
        // 56 <- s
        let data = &hex!("d30b02808083c5cdeb8783c5acfd9e407c565656")[..];

        let input_rlp = &mut &data[..];
        let res = PooledTransaction::decode(input_rlp);
        assert!(res.is_ok());
        assert!(input_rlp.is_empty());

        // we can also decode_enveloped
        let res = PooledTransaction::decode_2718(&mut &data[..]);
        assert!(res.is_ok());
    }

    #[test]
    fn decode_encode_raw_4844_rlp() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/4844rlp");
        let dir = std::fs::read_dir(path).expect("Unable to read folder");
        for entry in dir {
            let entry = entry.unwrap();
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let raw = hex::decode(content.trim()).unwrap();
            let tx = PooledTransaction::decode_2718(&mut raw.as_ref())
                .map_err(|err| {
                    panic!("Failed to decode transaction: {:?} {:?}", err, entry.path());
                })
                .unwrap();
            // We want to test only EIP-4844 transactions
            assert!(tx.is_eip4844());
            let encoded = tx.encoded_2718();
            assert_eq!(encoded.as_slice(), &raw[..], "{:?}", entry.path());
        }
    }

    #[test]
    #[cfg(feature = "kzg")]
    fn convert_to_eip7594() {
        let kzg_settings = alloy_eips::eip4844::env_settings::EnvKzgSettings::default();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/4844rlp");
        let dir = std::fs::read_dir(path).expect("Unable to read folder");
        for entry in dir {
            let entry = entry.unwrap();
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let raw = hex::decode(content.trim()).unwrap();
            let PooledTransaction::Eip4844(tx) = PooledTransaction::decode_2718(&mut raw.as_ref())
                .map_err(|err| {
                    panic!("Failed to decode transaction: {:?} {:?}", err, entry.path());
                })
                .unwrap()
            else {
                panic!("Expected EIP-4844 transaction");
            };
            let tx = tx.into_parts().0;
            assert!(!tx.sidecar.blobs().is_empty());
            assert!(tx.validate_blob(kzg_settings.get()).is_ok());

            let tx = tx
                .try_map_sidecar(|sidecar| {
                    sidecar.try_convert_into_eip7594_with_settings(kzg_settings.get())
                })
                .unwrap();

            assert!(!tx.sidecar.blobs().is_empty());
            assert!(tx.validate_blob(kzg_settings.get()).is_ok());
        }
    }
}
