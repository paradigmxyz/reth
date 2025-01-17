//! Transaction types.

use crate::RecoveredTx;
pub use alloy_consensus::transaction::PooledTransaction;
use once_cell as _;
pub use pooled::PooledTransactionsElementEcRecovered;
pub use reth_primitives_traits::{
    sync::{LazyLock, OnceLock},
    transaction::{
        error::{
            InvalidTransactionError, TransactionConversionError, TryFromRecoveredTransactionError,
        },
        signed::SignedTransactionIntoRecoveredExt,
    },
    FillTxEnv, WithEncoded,
};
pub use signature::{recover_signer, recover_signer_unchecked};
pub use tx_type::TxType;

/// Handling transaction signature operations, including signature recovery,
/// applying chain IDs, and EIP-2 validation.
pub mod signature;
pub mod util;

mod pooled;
mod tx_type;

/// Signed transaction.
pub use reth_ethereum_primitives::{Transaction, TransactionSigned};

/// Type alias kept for backward compatibility.
pub type TransactionSignedEcRecovered<T = TransactionSigned> = RecoveredTx<T>;

#[cfg(test)]
mod tests {
    use crate::{Transaction, TransactionSigned};
    use alloy_consensus::{Transaction as _, TxEip1559, TxLegacy};
    use alloy_eips::eip2718::{Decodable2718, Encodable2718};
    use alloy_primitives::{
        address, b256, bytes, hex, Address, Bytes, PrimitiveSignature as Signature, TxKind, B256,
        U256,
    };
    use alloy_rlp::{Decodable, Encodable, Error as RlpError};
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_codecs::Compact;
    use reth_primitives_traits::SignedTransaction;
    use std::str::FromStr;

    #[test]
    fn test_decode_empty_typed_tx() {
        let input = [0x80u8];
        let res = TransactionSigned::decode(&mut &input[..]).unwrap_err();
        assert_eq!(RlpError::InputTooShort, res);
    }

    #[test]
    fn raw_kind_encoding_sanity() {
        // check the 0x80 encoding for Create
        let mut buf = Vec::new();
        TxKind::Create.encode(&mut buf);
        assert_eq!(buf, vec![0x80]);

        // check decoding
        let buf = [0x80];
        let decoded = TxKind::decode(&mut &buf[..]).unwrap();
        assert_eq!(decoded, TxKind::Create);
    }

    #[test]
    fn test_decode_create_goerli() {
        // test that an example create tx from goerli decodes properly
        let tx_bytes = hex!("b901f202f901ee05228459682f008459682f11830209bf8080b90195608060405234801561001057600080fd5b50610175806100206000396000f3fe608060405234801561001057600080fd5b506004361061002b5760003560e01c80630c49c36c14610030575b600080fd5b61003861004e565b604051610045919061011d565b60405180910390f35b60606020600052600f6020527f68656c6c6f2073746174656d696e64000000000000000000000000000000000060405260406000f35b600081519050919050565b600082825260208201905092915050565b60005b838110156100be5780820151818401526020810190506100a3565b838111156100cd576000848401525b50505050565b6000601f19601f8301169050919050565b60006100ef82610084565b6100f9818561008f565b93506101098185602086016100a0565b610112816100d3565b840191505092915050565b6000602082019050818103600083015261013781846100e4565b90509291505056fea264697066735822122051449585839a4ea5ac23cae4552ef8a96b64ff59d0668f76bfac3796b2bdbb3664736f6c63430008090033c080a0136ebffaa8fc8b9fda9124de9ccb0b1f64e90fbd44251b4c4ac2501e60b104f9a07eb2999eec6d185ef57e91ed099afb0a926c5b536f0155dd67e537c7476e1471");

        let decoded = TransactionSigned::decode(&mut &tx_bytes[..]).unwrap();
        assert_eq!(tx_bytes.len(), decoded.length());
        assert_eq!(tx_bytes, &alloy_rlp::encode(decoded)[..]);
    }

    #[test]
    fn test_decode_recover_mainnet_tx() {
        // random mainnet tx <https://etherscan.io/tx/0x86718885c4b4218c6af87d3d0b0d83e3cc465df2a05c048aa4db9f1a6f9de91f>
        let tx_bytes = hex!("02f872018307910d808507204d2cb1827d0094388c818ca8b9251b393131c08a736a67ccb19297880320d04823e2701c80c001a0cf024f4815304df2867a1a74e9d2707b6abda0337d2d54a4438d453f4160f190a07ac0e6b3bc9395b5b9c8b9e6d77204a236577a5b18467b9175c01de4faa208d9");

        let decoded = TransactionSigned::decode_2718(&mut &tx_bytes[..]).unwrap();
        assert_eq!(
            decoded.recover_signer(),
            Some(Address::from_str("0x95222290DD7278Aa3Ddd389Cc1E1d165CC4BAfe5").unwrap())
        );
    }

    #[test]
    // Test vector from https://sepolia.etherscan.io/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    // Blobscan: https://sepolia.blobscan.com/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    fn test_decode_recover_sepolia_4844_tx() {
        use alloy_primitives::{address, b256};

        // https://sepolia.etherscan.io/getRawTx?tx=0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
        let raw_tx = alloy_primitives::hex::decode("0x03f9011d83aa36a7820fa28477359400852e90edd0008252089411e9ca82a3a762b4b5bd264d4173a242e7a770648080c08504a817c800f8a5a0012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921aa00152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4a0013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7a001148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1a0011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e654901a0c8de4cced43169f9aa3d36506363b2d2c44f6c49fc1fd91ea114c86f3757077ea01e11fdd0d1934eda0492606ee0bb80a7bf8f35cc5f86ec60fe5031ba48bfd544").unwrap();
        let decoded = TransactionSigned::decode_2718(&mut raw_tx.as_slice()).unwrap();
        assert!(alloy_consensus::Typed2718::is_eip4844(&decoded));

        let from = decoded.recover_signer();
        assert_eq!(from, Some(address!("A83C816D4f9b2783761a22BA6FADB0eB0606D7B2")));

        let tx = decoded.transaction;

        assert_eq!(tx.to(), Some(address!("11E9CA82A3a762b4B5bd264d4173a242e7a77064")));

        assert_eq!(
            tx.blob_versioned_hashes(),
            Some(
                &[
                    b256!("012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921a"),
                    b256!("0152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4"),
                    b256!("013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7"),
                    b256!("01148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1"),
                    b256!("011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e6549"),
                ][..]
            )
        );
    }

    #[test]
    fn decode_transaction_consumes_buffer() {
        let bytes = &mut &hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")[..];
        let _transaction_res = TransactionSigned::decode(bytes).unwrap();
        assert_eq!(
            bytes.len(),
            0,
            "did not consume all bytes in the buffer, {:?} remaining",
            bytes.len()
        );
    }

    #[test]
    fn decode_multiple_network_txs() {
        let bytes = hex!("f86b02843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba00eb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5aea03a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4u64),
            nonce: 2,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            false,
        );
        let hash = b256!("a517b206d2223278f860ea017d3626cacad4f52ff51030dc9a96b432f17f8d34");
        test_decode_and_encode(&bytes, transaction, signature, Some(hash));

        let bytes = hex!("f86b01843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac3960468702769bb01b2a00802ba0e24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0aa05406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 1u64,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(693361000000000u64),
            input: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xe24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0a")
                .unwrap(),
            U256::from_str("0x5406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da")
                .unwrap(),
            false,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f86b0384773594008398968094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba0ce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071a03ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 3,
            gas_price: 2000000000,
            gas_limit: 10000000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071")
                .unwrap(),
            U256::from_str("0x3ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88")
                .unwrap(),
            false,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469");
        let transaction = Transaction::Eip1559(TxEip1559 {
            chain_id: 4,
            nonce: 26,
            max_priority_fee_per_gas: 1500000000,
            max_fee_per_gas: 1500000013,
            gas_limit: MIN_TRANSACTION_GAS,
            to: Address::from_slice(&hex!("61815774383099e24810ab832a5b2a5425c154d5")[..]).into(),
            value: U256::from(3000000000000000000u64),
            input: Default::default(),
            access_list: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x59e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafd")
                .unwrap(),
            U256::from_str("0x016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")
                .unwrap(),
            true,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f8650f84832156008287fb94cf7f9e66af820a19257a2108375b180b0ec491678204d2802ca035b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981a0612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 15,
            gas_price: 2200000000,
            gas_limit: 34811,
            to: Address::from_slice(&hex!("cf7f9e66af820a19257a2108375b180b0ec49167")[..]).into(),
            value: U256::from(1234),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x35b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981")
                .unwrap(),
            U256::from_str("0x612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860")
                .unwrap(),
            true,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);
    }

    fn test_decode_and_encode(
        bytes: &[u8],
        transaction: Transaction,
        signature: Signature,
        hash: Option<B256>,
    ) {
        let expected = TransactionSigned::new_unhashed(transaction, signature);
        if let Some(hash) = hash {
            assert_eq!(hash, *expected.tx_hash());
        }
        assert_eq!(bytes.len(), expected.length());

        let decoded = TransactionSigned::decode(&mut &bytes[..]).unwrap();
        assert_eq!(expected, decoded);
        assert_eq!(bytes, &alloy_rlp::encode(expected));
    }

    #[test]
    fn decode_raw_tx_and_recover_signer() {
        use alloy_primitives::hex_literal::hex;
        // transaction is from ropsten

        let hash: B256 =
            hex!("559fb34c4a7f115db26cbf8505389475caaab3df45f5c7a0faa4abfa3835306c").into();
        let signer: Address = hex!("641c5d790f862a58ec7abcfd644c0442e9c201b3").into();
        let raw = hex!("f88b8212b085028fa6ae00830f424094aad593da0c8116ef7d2d594dd6a63241bccfc26c80a48318b64b000000000000000000000000641c5d790f862a58ec7abcfd644c0442e9c201b32aa0a6ef9e170bca5ffb7ac05433b13b7043de667fbb0b4a5e45d3b54fb2d6efcc63a0037ec2c05c3d60c5f5f78244ce0a3859e3a18a36c61efb061b383507d3ce19d2");

        let mut pointer = raw.as_ref();
        let tx = TransactionSigned::decode(&mut pointer).unwrap();
        assert_eq!(*tx.tx_hash(), hash, "Expected same hash");
        assert_eq!(tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }

    #[test]
    fn test_envelop_encode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode(&mut &input[..]).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded[..], input);
    }

    #[test]
    fn test_envelop_decode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = bytes!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode_2718(&mut input.as_ref()).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded, input);
    }

    #[test]
    fn test_decode_tx() {
        // some random transactions pulled from hive tests
        let data = hex!("b86f02f86c0705843b9aca008506fc23ac00830124f89400000000000000000000000000000000000003160180c001a00293c713e2f1eab91c366621ff2f867e05ad7e99d4aa5d069aafeb9e1e8c9b6aa05ec6c0605ff20b57c90a6484ec3b0509e5923733d06f9b69bee9a2dabe4f1352");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());

        let data = hex!("f865048506fc23ac00830124f8940000000000000000000000000000000000000316018032a06b8fdfdcb84790816b7af85b19305f493665fe8b4e7c51ffdd7cc144cd776a60a028a09ab55def7b8d6602ba1c97a0ebbafe64ffc9c8e89520cec97a8edfb2ebe9");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());
    }

    // <https://etherscan.io/tx/0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4>
    #[test]
    fn recover_legacy_singer() {
        let data = hex!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let tx = TransactionSigned::fallback_decode(&mut data.as_slice()).unwrap();
        assert!(tx.is_legacy());
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));
    }

    // <https://github.com/alloy-rs/alloy/issues/141>
    // <https://etherscan.io/tx/0xce4dc6d7a7549a98ee3b071b67e970879ff51b5b95d1c340bacd80fa1e1aab31>
    #[test]
    fn recover_enveloped() {
        let data = hex!("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));
        assert_eq!(tx.to(), Some(address!("D9e1459A7A482635700cBc20BBAF52D495Ab9C96")));
        assert_eq!(tx.input().as_ref(), hex!("1b55ba3a"));
        let encoded = tx.encoded_2718();
        assert_eq!(encoded.as_ref(), data.to_vec());
    }

    // <https://github.com/paradigmxyz/reth/issues/7750>
    // <https://etherscan.io/tx/0x2084b8144eea4031c2fa7dfe343498c5e665ca85ed17825f2925f0b5b01c36ac>
    #[test]
    fn recover_pre_eip2() {
        let data = hex!("f8ea0c850ba43b7400832dc6c0942935aa0a2d2fbb791622c29eb1c117b65b7a908580b884590528a9000000000000000000000001878ace42092b7f1ae1f28d16c1272b1aa80ca4670000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000d02ab486cedc0000000000000000000000000000000000000000000000000000557fe293cabc08cf1ca05bfaf3fda0a56b49cc78b22125feb5ae6a99d2b4781f00507d8b02c173771c85a0b5da0dbe6c5bc53740d0071fc83eb17ba0f709e49e9ae7df60dee625ef51afc5");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer();
        assert!(sender.is_none());
        let sender = tx.recover_signer_unchecked().unwrap();

        assert_eq!(sender, address!("7e9e359edf0dbacf96a9952fa63092d919b0842b"));
    }

    #[test]
    fn transaction_signed_no_hash_zstd_codec() {
        // will use same signature everywhere.
        // We don't need signature to match tx, just decoded to the same signature
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            false,
        );

        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![255],
            vec![1u8; 31],
            vec![255u8; 31],
            vec![1u8; 32],
            vec![255u8; 32],
            vec![1u8; 64],
            vec![255u8; 64],
        ];

        for input in inputs {
            let transaction = Transaction::Legacy(TxLegacy {
                chain_id: Some(4u64),
                nonce: 2,
                gas_price: 1000000000,
                gas_limit: 100000,
                to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
                value: U256::from(1000000000000000u64),
                input: Bytes::from(input),
            });

            let tx = TransactionSigned::new_unhashed(transaction, signature);
            test_transaction_signed_to_from_compact(tx);
        }
    }

    fn test_transaction_signed_to_from_compact(tx: TransactionSigned) {
        // zstd aware `to_compact`
        let mut buff: Vec<u8> = Vec::new();
        let written_bytes = tx.to_compact(&mut buff);
        let (decoded, _) = TransactionSigned::from_compact(&buff, written_bytes);
        assert_eq!(tx, decoded);
    }

    #[test]
    fn create_txs_disallowed_for_eip4844() {
        let data =
            [3, 208, 128, 128, 123, 128, 120, 128, 129, 129, 128, 192, 129, 129, 192, 128, 128, 9];
        let res = TransactionSigned::decode_2718(&mut &data[..]);

        assert!(res.is_err());
    }
}
