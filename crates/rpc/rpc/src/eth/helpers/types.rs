//! L1 `eth` API types.

use alloy_network::Ethereum;
use reth_evm_ethereum::EthEvmConfig;
use reth_rpc_convert::RpcConverter;
use reth_rpc_eth_types::receipt::EthReceiptConverter;

/// An [`RpcConverter`] with its generics set to Ethereum specific.
pub type EthRpcConverter<ChainSpec> =
    RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>;

//tests for simulate
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxEip8141, TxType};
    use alloy_eips::{
        eip8141::{Frame, FrameLimits, FrameMode, TransactionFees},
        eip8250::{nonce_slot, NONCE_MANAGER},
    };
    use alloy_primitives::{Address, B256, U256};
    use alloy_rpc_types_eth::TransactionRequest;
    use evm2::evm::{AccountInfo, EmptyDB, InMemoryDB};
    use reth_chainspec::MAINNET;
    use reth_evm_ethereum::EthEvmEnv;
    use reth_rpc_convert::RpcConvert;
    use reth_rpc_eth_types::simulate::resolve_transaction;

    #[test]
    fn test_resolve_transaction_empty_request() {
        let builder = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        let mut db = EmptyDB::default();
        let tx = TransactionRequest::default();
        let result = resolve_transaction(tx, 21000, 0, 1, false, &mut db, &builder).unwrap();

        // For an empty request, we should get a valid transaction with defaults
        let tx = result.into_inner();
        assert_eq!(tx.max_fee_per_gas(), 0);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(0));
        assert_eq!(tx.gas_price(), None);
    }

    #[test]
    fn test_resolve_transaction_legacy() {
        let mut db = EmptyDB::default();
        let builder = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));

        let tx = TransactionRequest { gas_price: Some(100), ..Default::default() };

        let tx = resolve_transaction(tx, 21000, 0, 1, false, &mut db, &builder).unwrap();

        assert_eq!(tx.tx_type(), TxType::Legacy);

        let tx = tx.into_inner();
        assert_eq!(tx.gas_price(), Some(100));
        assert_eq!(tx.max_priority_fee_per_gas(), None);
    }

    #[test]
    fn test_resolve_transaction_partial_eip1559() {
        let mut db = EmptyDB::default();
        let rpc_converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));

        let tx = TransactionRequest {
            max_fee_per_gas: Some(200),
            max_priority_fee_per_gas: Some(10),
            ..Default::default()
        };

        let result = resolve_transaction(tx, 21000, 0, 1, false, &mut db, &rpc_converter).unwrap();

        assert_eq!(result.tx_type(), TxType::Eip1559);
        let tx = result.into_inner();
        assert_eq!(tx.max_fee_per_gas(), 200);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(10));
        assert_eq!(tx.gas_price(), None);
    }

    #[test]
    fn test_resolve_transaction_wraps_max_nonce_when_nonce_check_disabled() {
        let mut db = EmptyDB::default();
        let rpc_converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));

        let tx = TransactionRequest { nonce: Some(u64::MAX), ..Default::default() };

        let result = resolve_transaction(tx, 21000, 0, 1, true, &mut db, &rpc_converter).unwrap();

        assert_eq!(result.nonce(), 0);
    }

    #[test]
    fn test_typed_request_fee_defaults() {
        let converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        // Default blob fee caps must retain values beyond u64 without truncation.
        for blob_fee in [u128::from(u64::MAX), u128::from(u64::MAX) + 1, u128::MAX] {
            let mut env = EthEvmEnv::default();
            env.block.blob_basefee = U256::from(blob_fee);
            env.block.basefee = U256::from(30);
            let request = TransactionRequest {
                to: Some(Address::repeat_byte(0xaa).into()),
                blob_versioned_hashes: Some(vec![B256::repeat_byte(1)]),
                gas_price: Some(100),
                ..Default::default()
            };

            let tx = converter.tx_env(request.clone(), &env).unwrap();

            assert_eq!(tx.max_fee_per_blob_gas(), Some(blob_fee));
            // Flat gasPrice must not be reduced to the base fee for a blob request.
            assert_eq!(tx.effective_gas_price(Some(30)), 100);

            // An authorization list selects EIP-7702, which must preserve the same flat price.
            let request = TransactionRequest {
                authorization_list: Some(vec![]),
                blob_versioned_hashes: None,
                ..request
            };
            let tx = converter.tx_env(request.clone(), &env).unwrap();
            assert_eq!(tx.effective_gas_price(Some(30)), 100);

            // An explicit zero priority fee still opts into base-fee pricing.
            let request = TransactionRequest {
                gas_price: None,
                max_fee_per_gas: Some(100),
                max_priority_fee_per_gas: Some(0),
                ..request
            };
            let tx = converter.tx_env(request, &env).unwrap();
            assert_eq!(tx.effective_gas_price(Some(30)), 30);
        }
    }

    fn frame_transaction() -> TxEip8141 {
        TxEip8141 {
            chain_id: 1,
            sender: Address::repeat_byte(0xaa),
            nonce: 9,
            nonce_keys: Some(vec![U256::ZERO]),
            frames: vec![Frame {
                mode: FrameMode::Verify,
                flags: 3,
                limits: FrameLimits { execution: 45_000, state: 7_000 },
                ..Default::default()
            }],
            fees: TransactionFees {
                max_fee: U256::ONE << 220,
                max_priority_fee_per_gas: U256::ONE << 180,
            },
            ..Default::default()
        }
    }

    #[test]
    fn frame_env_preserves_full_width_fees_and_independent_limits() {
        let converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        let expected = frame_transaction();
        let request = <TransactionRequest as From<_>>::from(expected.clone());
        let tx = converter.tx_env(request, &EthEvmEnv::default()).unwrap();
        assert_eq!(tx.frame_transaction(), Some(&expected));
        assert_eq!(tx.signer(), expected.sender);
    }

    #[test]
    fn frame_env_rejects_fields_outside_the_canonical_envelope() {
        let converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        let cases: &[fn(&mut TransactionRequest)] = &[
            |request| request.gas = Some(1),
            |request| request.gas_price = Some(0),
            |request| request.max_fee_per_gas = Some(0),
            |request| request.max_fee_per_blob_gas = Some(0),
            |request| request.to = Some(Address::ZERO.into()),
            |request| request.value = Some(U256::ONE),
            |request| request.access_list = Some(Default::default()),
            |request| request.authorization_list = Some(vec![]),
            |request| request.input = alloy_rpc_types_eth::TransactionInput::new(vec![1].into()),
            |request| request.transaction_type = Some(u8::from(TxType::Legacy)),
        ];
        for prepare in cases {
            let mut request = <TransactionRequest as From<_>>::from(frame_transaction());
            prepare(&mut request);
            assert_eq!(
                converter.tx_env(request, &EthEvmEnv::default()).unwrap_err().to_string(),
                "EIP-8141 transaction request contains non-canonical outer fields"
            );
        }
    }

    #[test]
    fn frame_env_rejects_invalid_counts_before_converting_frames() {
        let converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        for count in [0, alloy_eips::eip8141::MAX_FRAMES + 1] {
            let mut request = <TransactionRequest as From<_>>::from(frame_transaction());
            request.frames = Some(vec![Frame::default().into(); count]);
            assert_eq!(
                converter.tx_env(request, &EthEvmEnv::default()).unwrap_err().to_string(),
                "expected between 1 and 64 frames"
            );
        }
    }

    #[test]
    fn simulated_frames_resolve_keyed_nonce_and_preserve_single_use_sequence() {
        let converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        let payload = frame_transaction();
        let mut db = InMemoryDB::default();
        db.insert_account_info(&payload.sender, AccountInfo::default().with_nonce(77));
        db.insert_account_storage(
            &NONCE_MANAGER,
            &nonce_slot(payload.sender, U256::ZERO),
            &U256::from(9),
        );
        let mut request = <TransactionRequest as From<_>>::from(payload.clone());
        request.nonce = None;
        let resolved =
            resolve_transaction(request, 30_000_000, 0, 1, false, &mut db, &converter).unwrap();
        assert_eq!(resolved.frame_transaction(), Some(&payload));
        let mut single_use = payload;
        single_use.nonce = u64::MAX;
        let request = <TransactionRequest as From<_>>::from(single_use.clone());
        let resolved =
            resolve_transaction(request, 30_000_000, 0, 1, true, &mut db, &converter).unwrap();
        assert_eq!(resolved.frame_transaction(), Some(&single_use));
    }
}
