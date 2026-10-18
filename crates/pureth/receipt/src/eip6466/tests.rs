use super::*;
use alloy_consensus::{Header, TxEip7702, TxLegacy};
use alloy_primitives::{b256, hex, Log as ExecutionLog, Signature, TxKind};
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_ethereum_primitives::{BlockBody, Transaction as EthereumTransaction};

fn chain_spec() -> ChainSpec {
    ChainSpecBuilder::mainnet().byzantium_activated().build()
}

#[test]
fn amsterdam_receipt_charges_are_not_block_resource_gas() {
    let block = recovered_block(
        vec![
            legacy_transaction(0, TxKind::Call(Address::ZERO)),
            legacy_transaction(1, TxKind::Call(Address::ZERO)),
        ],
        vec![address(1), address(2)],
        90_000,
        100_000,
    );
    let stored = [
        stored_receipt(TxType::Legacy, true, 70_000, Vec::new()),
        stored_receipt(TxType::Legacy, true, 140_000, Vec::new()),
    ];
    assert!(matches!(
        receipts_from_block(&block, &stored, &chain_spec()),
        Err(ReceiptConstructionError::CumulativeGasExceedsBlockLimit { index: 1, .. })
    ));
    let spec = ChainSpecBuilder::mainnet().amsterdam_activated().build();
    let receipts = receipts_from_block(&block, &stored, &spec).unwrap();
    assert_eq!(receipts.get(0).unwrap().gas_used(), 70_000);
    assert_eq!(receipts.get(1).unwrap().gas_used(), 70_000);
}

#[test]
fn signed_gas_limits_and_empty_block_gas_still_apply_in_both_forks() {
    for spec in [chain_spec(), ChainSpecBuilder::mainnet().amsterdam_activated().build()] {
        let block = recovered_block(
            vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
            vec![address(1)],
            21_000,
            2_000_000,
        );
        let stored = [stored_receipt(TxType::Legacy, true, 1_000_001, Vec::new())];
        assert_eq!(
            receipts_from_block(&block, &stored, &spec),
            Err(ReceiptConstructionError::TransactionGasUsedExceedsLimit {
                index: 0,
                gas_used: 1_000_001,
                gas_limit: 1_000_000
            })
        );
        assert_eq!(
            receipts_from_block(&recovered_block(Vec::new(), Vec::new(), 1, 1), &[], &spec),
            Err(ReceiptConstructionError::BlockGasUsedMismatch { header: 1, receipts: 0 })
        );
    }
}

#[test]
fn codec_roundtrips_receipt_variants_and_lists() {
    let mut variants = receipt_variants();
    for receipt in &mut variants {
        let log = Log::new(
            address(0x11),
            vec![B256::repeat_byte(0x22), B256::repeat_byte(0x33)],
            Bytes::from(vec![1, 2, 3]),
        )
        .unwrap();
        match receipt {
            Receipt::Basic(receipt) => receipt.logs.push(log),
            Receipt::Create(receipt) => receipt.logs.push(log),
            Receipt::SetCode(receipt) => receipt.logs.push(log),
        }
        let bytes = receipt.to_ssz_bytes().unwrap();
        let decoded = Receipt::from_ssz_bytes(&bytes).unwrap();
        assert_eq!(decoded, *receipt);
        assert_eq!(decoded.to_ssz_bytes().unwrap(), bytes);
        let log = &receipt.logs()[0];
        assert_eq!(Log::from_ssz_bytes(&log.to_ssz_bytes().unwrap()).unwrap(), *log);
    }

    for receipts in [Receipts::new(Vec::new()), Receipts::new(variants.to_vec())] {
        let bytes = receipts.to_ssz_bytes().unwrap();
        let decoded = Receipts::from_ssz_bytes(&bytes).unwrap();
        assert_eq!(decoded, receipts);
        assert_eq!(decoded.to_ssz_bytes().unwrap(), bytes);
    }
}

#[test]
fn codec_rejects_noncanonical_receipt_encodings() {
    assert!(Receipt::from_ssz_bytes(&[]).is_err());
    let basic = receipt_variants()[0].to_ssz_bytes().unwrap();
    assert_eq!(Receipt::from_ssz_bytes(&basic).unwrap(), receipt_variants()[0]);
    for selector in [0, 4, 127, 255] {
        let mut bytes = basic.clone();
        bytes[0] = selector;
        assert_eq!(
            Receipt::from_ssz_bytes(&bytes),
            Err(ssz::DecodeError::UnionSelectorInvalid(selector))
        );
    }

    let mut invalid_bool = basic.clone();
    invalid_bool[33] = 2;
    assert!(Receipt::from_ssz_bytes(&invalid_bool).is_err());

    for offset in [0_u32, 32, 34, u32::MAX] {
        let mut bytes = basic.clone();
        bytes[29..33].copy_from_slice(&offset.to_le_bytes());
        assert!(Receipt::from_ssz_bytes(&bytes).is_err());
    }

    let mut authority = receipt_variants()[2].to_ssz_bytes().unwrap();
    authority.pop();
    assert!(Receipt::from_ssz_bytes(&authority).is_err());

    let mut list = Receipts::new(receipt_variants().to_vec()).to_ssz_bytes().unwrap();
    list[..4].copy_from_slice(&0_u32.to_le_bytes());
    assert!(Receipts::from_ssz_bytes(&list).is_err());
    list[..4].copy_from_slice(&13_u32.to_le_bytes());
    assert!(Receipts::from_ssz_bytes(&list).is_err());
}

#[test]
fn codec_checks_topic_width_and_bound_before_decoding() {
    for topic_bytes in [31_usize, 160] {
        let mut bytes = vec![0_u8; 28 + topic_bytes];
        bytes[20..24].copy_from_slice(&28_u32.to_le_bytes());
        bytes[24..28]
            .copy_from_slice(&(28_u32 + u32::try_from(topic_bytes).unwrap()).to_le_bytes());
        assert!(Log::from_ssz_bytes(&bytes).is_err());
    }
    let log = Log::new(address(1), vec![B256::ZERO; 4], Bytes::new()).unwrap();
    assert_eq!(Log::from_ssz_bytes(&log.to_ssz_bytes().unwrap()).unwrap(), log);
}

#[test]
fn receipt_accessors_preserve_variant_presence() {
    let [basic, mut create, mut set_code] = receipt_variants();
    if let Receipt::Create(receipt) = &mut create {
        receipt.status = false;
        receipt.contract_address = Address::ZERO;
    }
    if let Receipt::SetCode(receipt) = &mut set_code {
        receipt.authorities.clear();
    }
    assert_eq!(basic.from_(), address(1));
    assert_eq!(basic.gas_used(), 21_000);
    assert!(basic.status());
    assert_eq!(basic.contract_address(), None);
    assert_eq!(basic.authorities(), None);
    assert_eq!(create.contract_address(), Some(Address::ZERO));
    assert!(!create.status());
    assert_eq!(set_code.authorities(), Some([].as_slice()));
    for receipt in [basic, create, set_code] {
        assert_eq!(Receipt::from_ssz_bytes(&receipt.to_ssz_bytes().unwrap()).unwrap(), receipt);
    }
}

#[test]
fn historical_acquisition_recovers_set_code_and_mixed_block_authorities() {
    let (set_code, stored) = set_code_block_fixture(false);
    let mixed = recovered_block(
        vec![
            legacy_transaction(0, TxKind::Call(address(2))),
            set_code.body().transactions[0].clone(),
        ],
        vec![address(1), address(0x61)],
        51_000,
        100_000,
    );
    let mixed_stored = vec![
        stored_receipt(TxType::Legacy, true, 21_000, Vec::new()),
        stored_receipt(TxType::Eip7702, false, 51_000, Vec::new()),
    ];
    for (block, receipts) in [(set_code, stored.to_vec()), (mixed, mixed_stored)] {
        let expected =
            Eip6466ReceiptSnapshot::from_block(&block, &receipts, &chain_spec()).unwrap();
        let (provider, hash) = crate::test_utils::historical_provider_for_block(
            block,
            Some(receipts),
            std::sync::Arc::new(
                reth_chainspec::ChainSpecBuilder::mainnet().byzantium_activated().build(),
            ),
        );
        let snapshot = crate::ProviderSnapshot::from_reth_historical(&provider, hash).unwrap();
        assert_eq!(snapshot.receipt_snapshot().serialized(), expected.serialized());
        let children = snapshot.receipt_snapshot().tree().children().unwrap().as_ptr();
        let receipts = snapshot.into_receipt_snapshot();
        assert_eq!(receipts.tree().children().unwrap().as_ptr(), children);
        assert_eq!(
            receipts.receipts().as_slice().last().unwrap().authorities().unwrap(),
            &[
                Address::from(hex!("4fd357b597c2d9c930a24645958f8cbc43a11d2e")),
                Address::from(hex!("fc8ceb2413f8f3808eab57499da4a8b5179f6820")),
            ]
        );
    }
}

#[test]
fn historical_acquisition_respects_byzantium_activation() {
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(address(2)))],
        vec![address(1)],
        21_000,
        30_000,
    );
    let (provider, hash) = crate::test_utils::historical_provider_for_block(
        block,
        Some(vec![stored_receipt(TxType::Legacy, true, 21_000, Vec::new())]),
        reth_chainspec::MAINNET.clone(),
    );
    assert!(matches!(
        crate::ProviderSnapshot::from_reth_historical(&provider, hash),
        Err(crate::HistoricalAcquisitionError::Snapshot(Eip6466SnapshotError::MissingData(
            ReceiptConstructionError::PreEip658Block
        )))
    ));
}

fn address(byte: u8) -> Address {
    Address::from([byte; 20])
}

#[test]
fn historical_acquisition_preserves_successful_and_failed_create_receipts() {
    let sender = address(0x61);
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Create), legacy_transaction(1, TxKind::Create)],
        vec![sender, sender],
        106_000,
        120_000,
    );
    let (provider, hash) = crate::test_utils::historical_provider_for_block(
        block,
        Some(vec![
            stored_receipt(TxType::Legacy, true, 53_000, Vec::new()),
            stored_receipt(TxType::Legacy, false, 106_000, Vec::new()),
        ]),
        std::sync::Arc::new(
            reth_chainspec::ChainSpecBuilder::mainnet().byzantium_activated().build(),
        ),
    );
    let result = crate::ProviderSnapshot::from_reth_historical(&provider, hash).unwrap();
    let snapshot = result.receipt_snapshot();
    let receipts = snapshot.receipts();
    assert_eq!(receipts.get(0).unwrap().contract_address(), Some(sender.create(0)));
    assert_eq!(receipts.get(1).unwrap().contract_address(), Some(Address::ZERO));
    assert!(!receipts.get(1).unwrap().status());
    assert_eq!(result.root(), snapshot.tree().root());
    assert_eq!(Receipts::from_ssz_bytes(snapshot.serialized()).unwrap(), *receipts);
}

fn recovered_block(
    transactions: Vec<TransactionSigned>,
    senders: Vec<Address>,
    gas_used: u64,
    gas_limit: u64,
) -> RecoveredBlock<Block> {
    assert_eq!(transactions.len(), senders.len());

    RecoveredBlock::try_new_unhashed(
        Block {
            header: Header { gas_used, gas_limit, ..Default::default() },
            body: BlockBody { transactions, ..Default::default() },
        },
        senders,
    )
    .unwrap()
}

fn legacy_transaction(nonce: u64, to: TxKind) -> TransactionSigned {
    TransactionSigned::new_unhashed(
        EthereumTransaction::Legacy(TxLegacy {
            nonce,
            to,
            gas_limit: 1_000_000,
            ..Default::default()
        }),
        Signature::test_signature(),
    )
}

fn set_code_transaction() -> TxEip7702 {
    TxEip7702 {
        chain_id: 1,
        gas_limit: 1_000_000,
        authorization_list: serde_json::from_value(serde_json::json!([
            {
                "chainId": "0x0",
                "address": "0x3031323334353637383940414243444546474849",
                "nonce": "0x0",
                "yParity": "0x1",
                "r": "0xa4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d454",
                "s": "0x28aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f6"
            },
            {
                "chainId": "0x1",
                "address": "0x5051525354555657585960616263646566676869",
                "nonce": "0x309",
                "yParity": "0x1",
                "r": "0xa4be86c16c6d3a2b907660b24187d0b30b69f6db3e6e8e7a7bb1183a4706d454",
                "s": "0x28aba84cdee6059dde41620422959d01da4f6cfff21a9b97036db018f1d815f6"
            }
        ]))
        .unwrap(),
        ..Default::default()
    }
}

fn stored_receipt(
    tx_type: TxType,
    success: bool,
    cumulative_gas_used: u64,
    logs: Vec<ExecutionLog>,
) -> StoredReceipt {
    StoredReceipt { tx_type, success, cumulative_gas_used, logs }
}

fn execution_log(address: Address, topics: Vec<B256>, data: &[u8]) -> ExecutionLog {
    ExecutionLog::new_unchecked(address, topics, Bytes::from(data.to_vec()))
}

fn receipt_variants() -> [Receipt; 3] {
    [
        Receipt::Basic(BasicReceipt {
            from_: address(1),
            gas_used: 21_000,
            logs: Vec::new(),
            status: true,
        }),
        Receipt::Create(CreateReceipt {
            from_: address(1),
            gas_used: 53_000,
            contract_address: address(2),
            logs: Vec::new(),
            status: true,
        }),
        Receipt::SetCode(SetCodeReceipt {
            from_: address(1),
            gas_used: 30_000,
            logs: Vec::new(),
            status: false,
            authorities: vec![address(2), Address::ZERO],
        }),
    ]
}

fn set_code_block_fixture(success: bool) -> (RecoveredBlock<Block>, [StoredReceipt; 1]) {
    let transaction = TransactionSigned::new_unhashed(
        EthereumTransaction::Eip7702(set_code_transaction()),
        Signature::test_signature(),
    );

    let block = recovered_block(vec![transaction], vec![address(0x61)], 30_000, 60_000);

    let stored = [stored_receipt(TxType::Eip7702, success, 30_000, Vec::new())];

    (block, stored)
}

#[test]
fn compatible_union_selectors_are_pinned() {
    let [basic, create, set_code] = receipt_variants();

    assert_eq!(basic.selector(), BASIC_RECEIPT_SELECTOR);
    assert_eq!(create.selector(), CREATE_RECEIPT_SELECTOR);
    assert_eq!(set_code.selector(), SET_CODE_RECEIPT_SELECTOR);
}

#[test]
fn progressive_active_fields_are_pinned() {
    let [basic, create, set_code] = receipt_variants();

    assert_eq!(basic.active_fields(), BASIC_RECEIPT_ACTIVE_FIELDS);
    assert_eq!(create.active_fields(), CREATE_RECEIPT_ACTIVE_FIELDS);
    assert_eq!(set_code.active_fields(), SET_CODE_RECEIPT_ACTIVE_FIELDS);
}

#[test]
fn transaction_types_are_classified_semantically() {
    assert_eq!(classify_receipt(TxType::Legacy, false).unwrap(), ReceiptKind::Basic);
    assert_eq!(classify_receipt(TxType::Eip2930, false).unwrap(), ReceiptKind::Basic);
    assert_eq!(classify_receipt(TxType::Eip1559, false).unwrap(), ReceiptKind::Basic);
    assert_eq!(classify_receipt(TxType::Eip4844, false).unwrap(), ReceiptKind::Basic);
    assert_eq!(classify_receipt(TxType::Legacy, true).unwrap(), ReceiptKind::Create);
    assert_eq!(classify_receipt(TxType::Eip2930, true).unwrap(), ReceiptKind::Create);
    assert_eq!(classify_receipt(TxType::Eip1559, true).unwrap(), ReceiptKind::Create);
    assert_eq!(classify_receipt(TxType::Eip7702, false).unwrap(), ReceiptKind::SetCode);
}

#[test]
fn impossible_create_kinds_are_rejected() {
    assert_eq!(
        classify_receipt(TxType::Eip4844, true),
        Err(ReceiptConstructionError::UnsupportedTransaction {
            tx_type: TxType::Eip4844,
            is_create: true,
        })
    );
    assert_eq!(
        classify_receipt(TxType::Eip7702, true),
        Err(ReceiptConstructionError::UnsupportedTransaction {
            tx_type: TxType::Eip7702,
            is_create: true,
        })
    );
}

#[test]
fn log_preserves_address_topics_and_data() {
    let address = address(7);
    let topics = vec![B256::from([1; 32]), B256::from([2; 32])];
    let data = Bytes::from(vec![0xaa, 0xbb, 0xcc]);

    let log = Log::new(address, topics.clone(), data.clone()).unwrap();

    assert_eq!(log.address(), address);
    assert_eq!(log.topics(), topics);
    assert_eq!(log.data(), &data);
}

#[test]
fn log_accepts_zero_and_four_topics() {
    assert!(Log::new(Address::ZERO, Vec::new(), Bytes::new()).is_ok());
    assert!(Log::new(Address::ZERO, vec![B256::ZERO; 4], Bytes::new()).is_ok());
}

#[test]
fn log_rejects_five_topics_without_truncating() {
    assert_eq!(
        Log::new(Address::ZERO, vec![B256::ZERO; 5], Bytes::new()),
        Err(ReceiptConstructionError::TooManyTopics { actual: 5, max: 4 })
    );
}

#[test]
fn reth_log_conversion_preserves_all_fields() {
    let address = address(9);
    let topics = vec![B256::from([1; 32]), B256::from([2; 32])];
    let data = Bytes::from(vec![0x01, 0x02, 0x03]);
    let recovered_log = ExecutionLog::new_unchecked(address, topics.clone(), data.clone());

    let log = Log::try_from(&recovered_log).unwrap();

    assert_eq!(log.address(), address);
    assert_eq!(log.topics(), topics);
    assert_eq!(log.data(), &data);
}

#[test]
fn block_conversion_preserves_senders_gas_status_and_logs() {
    let first_sender = address(0x11);
    let second_sender = address(0x12);
    let first_log_address = address(0x21);
    let second_log_address = address(0x22);

    let block = recovered_block(
        vec![
            legacy_transaction(0, TxKind::Call(Address::ZERO)),
            legacy_transaction(1, TxKind::Call(Address::ZERO)),
        ],
        vec![first_sender, second_sender],
        71_000,
        100_000,
    );

    let stored_receipts = [
        stored_receipt(
            TxType::Legacy,
            true,
            21_000,
            vec![execution_log(first_log_address, vec![B256::repeat_byte(0x31)], &[0x01, 0x02])],
        ),
        stored_receipt(
            TxType::Legacy,
            false,
            71_000,
            vec![execution_log(second_log_address, vec![B256::repeat_byte(0x32)], &[0x03, 0x04])],
        ),
    ];

    let converted = receipts_from_block(&block, &stored_receipts, &chain_spec()).unwrap();

    assert_eq!(converted.len(), 2);

    match converted.get(0).unwrap() {
        Receipt::Basic(receipt) => {
            assert_eq!(receipt.from_, first_sender);
            assert_eq!(receipt.gas_used, 21_000);
            assert!(receipt.status);
            assert_eq!(receipt.logs[0].address(), first_log_address);
            assert_eq!(receipt.logs[0].topics(), &[B256::repeat_byte(0x31)]);
            assert_eq!(receipt.logs[0].data(), &Bytes::from(vec![0x01, 0x02]));
        }
        receipt => panic!("expected Basic receipt, got {receipt:?}"),
    }

    match converted.get(1).unwrap() {
        Receipt::Basic(receipt) => {
            assert_eq!(receipt.from_, second_sender);
            assert_eq!(receipt.gas_used, 50_000);
            assert!(!receipt.status);
            assert_eq!(receipt.logs[0].address(), second_log_address);
            assert_eq!(receipt.logs[0].topics(), &[B256::repeat_byte(0x32)]);
            assert_eq!(receipt.logs[0].data(), &Bytes::from(vec![0x03, 0x04]));
        }
        receipt => panic!("expected Basic receipt, got {receipt:?}"),
    }
}

#[test]
fn failed_create_has_zero_contract_address() {
    let sender = address(0x41);
    let nonce = 7;

    let block = recovered_block(
        vec![legacy_transaction(nonce, TxKind::Create)],
        vec![sender],
        53_000,
        60_000,
    );

    let stored_receipts = [stored_receipt(TxType::Legacy, false, 53_000, Vec::new())];

    let snapshot =
        Eip6466ReceiptSnapshot::from_block(&block, &stored_receipts, &chain_spec()).unwrap();
    let converted = snapshot.receipts();

    match converted.get(0).unwrap() {
        Receipt::Create(receipt) => {
            assert_eq!(receipt.from_, sender);
            assert_eq!(receipt.contract_address, Address::ZERO);
            assert_eq!(receipt.gas_used, 53_000);
            assert!(!receipt.status);
        }
        receipt => panic!("expected Create receipt, got {receipt:?}"),
    }
    assert_eq!(
        snapshot.serialized().as_ref(),
        hex!(
            "0400000002\
         4141414141414141414141414141414141414141\
         08cf000000000000\
         0000000000000000000000000000000000000000\
         3500000000"
        )
    );
    assert_eq!(
        snapshot.root(),
        b256!("6c2cdc5fa4ae92952703c12ad5a3e11788197c27373c44ed1bd985bbb424c0c1")
    );
    assert_eq!(snapshot.root(), snapshot.tree().root());
}

#[test]
fn successful_create_keeps_derived_contract_address() {
    let sender = address(0x41);
    let block =
        recovered_block(vec![legacy_transaction(7, TxKind::Create)], vec![sender], 53_000, 60_000);
    let stored = [stored_receipt(TxType::Legacy, true, 53_000, Vec::new())];
    let converted = receipts_from_block(&block, &stored, &chain_spec()).unwrap();
    let Receipt::Create(receipt) = converted.get(0).unwrap() else {
        panic!("expected Create receipt");
    };
    assert_eq!(receipt.contract_address, sender.create(7));
    assert!(receipt.status);
}

#[test]
fn recovered_authorities_do_not_depend_on_chain_or_nonce_acceptance() {
    for (field, value) in [("chainId", "0x2"), ("nonce", "0xffffffffffffffff")] {
        let mut payload = set_code_transaction();
        let mut authorization = serde_json::to_value(&payload.authorization_list[0]).unwrap();
        authorization[field] = serde_json::json!(value);
        payload.authorization_list[0] = serde_json::from_value(authorization).unwrap();
        let expected = payload.authorization_list[0].recover_authority().unwrap();
        let transaction = TransactionSigned::new_unhashed(
            EthereumTransaction::Eip7702(payload),
            Signature::test_signature(),
        );
        let receipt =
            receipt_from_transaction(&transaction, address(1), 30_000, Vec::new(), true).unwrap();
        assert_eq!(receipt.authorities().unwrap()[0], expected);
        assert_ne!(expected, Address::ZERO);
    }
}

#[test]
fn invalid_inner_signatures_commit_zero_without_losing_other_positions() {
    for (field, value) in [
        ("r", "0x0"),
        ("s", "0x0"),
        ("yParity", "0x2"),
        ("s", "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    ] {
        let mut payload = set_code_transaction();
        let mut authorization = serde_json::to_value(&payload.authorization_list[0]).unwrap();
        authorization[field] = serde_json::json!(value);
        payload.authorization_list[0] = serde_json::from_value(authorization).unwrap();
        assert!(payload.authorization_list[0].recover_authority().is_err());
        let transaction = TransactionSigned::new_unhashed(
            EthereumTransaction::Eip7702(payload),
            Signature::test_signature(),
        );
        let receipt =
            receipt_from_transaction(&transaction, address(1), 30_000, Vec::new(), false).unwrap();
        assert_eq!(
            receipt.authorities().unwrap(),
            &[Address::ZERO, Address::from(hex!("fc8ceb2413f8f3808eab57499da4a8b5179f6820"))]
        );
    }
}

#[test]
fn empty_authorization_list_is_a_conversion_failure() {
    let transaction = TransactionSigned::new_unhashed(
        EthereumTransaction::Eip7702(TxEip7702::default()),
        Signature::test_signature(),
    );
    assert_eq!(
        receipt_from_transaction(&transaction, address(1), 30_000, Vec::new(), true),
        Err(ReceiptConstructionError::EmptyAuthorizationList)
    );
}

#[test]
fn equal_cumulative_gas_produces_zero_individual_gas() {
    let block = recovered_block(
        vec![
            legacy_transaction(0, TxKind::Call(Address::ZERO)),
            legacy_transaction(1, TxKind::Call(Address::ZERO)),
        ],
        vec![address(1), address(2)],
        21_000,
        30_000,
    );

    let stored_receipts = [
        stored_receipt(TxType::Legacy, true, 21_000, Vec::new()),
        stored_receipt(TxType::Legacy, true, 21_000, Vec::new()),
    ];

    let converted = receipts_from_block(&block, &stored_receipts, &chain_spec()).unwrap();

    match converted.get(1).unwrap() {
        Receipt::Basic(receipt) => assert_eq!(receipt.gas_used, 0),
        receipt => panic!("expected Basic receipt, got {receipt:?}"),
    }
}

#[test]
fn block_conversion_rejects_decreasing_cumulative_gas() {
    let block = recovered_block(
        vec![
            legacy_transaction(0, TxKind::Call(Address::ZERO)),
            legacy_transaction(1, TxKind::Call(Address::ZERO)),
        ],
        vec![address(1), address(2)],
        50_000,
        60_000,
    );

    let stored_receipts = [
        stored_receipt(TxType::Legacy, true, 50_000, Vec::new()),
        stored_receipt(TxType::Legacy, true, 40_000, Vec::new()),
    ];

    let error = receipts_from_block(&block, &stored_receipts, &chain_spec()).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::DecreasingCumulativeGas {
            index: 1,
            previous: 50_000,
            current: 40_000,
        }
    );
}

#[test]
fn block_conversion_rejects_input_count_mismatch() {
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![address(1)],
        21_000,
        30_000,
    );

    let error = receipts_from_block(&block, &[], &chain_spec()).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::InputCountMismatch { transactions: 1, receipts: 0 }
    );

    let stored = [stored_receipt(TxType::Legacy, true, 21_000, Vec::new())];
    for spec in [chain_spec(), ChainSpecBuilder::mainnet().amsterdam_activated().build()] {
        for senders in [Vec::new(), vec![address(1), address(2)]] {
            let count = senders.len();
            let malformed = RecoveredBlock::new_unhashed(block.clone_block(), senders);
            let error = receipts_from_block(&malformed, &stored, &spec).unwrap_err();
            assert_eq!(
                error,
                ReceiptConstructionError::SenderCountMismatch { transactions: 1, senders: count }
            );
            assert_eq!(
                matches!(
                    Eip6466SnapshotError::from_construction(error),
                    Eip6466SnapshotError::MissingData(_)
                ),
                count < 1
            );
        }
    }
}

#[test]
fn block_conversion_rejects_transaction_type_mismatch() {
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![address(1)],
        21_000,
        30_000,
    );

    let stored_receipts = [stored_receipt(TxType::Eip1559, true, 21_000, Vec::new())];

    let error = receipts_from_block(&block, &stored_receipts, &chain_spec()).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::TransactionTypeMismatch {
            index: 0,
            transaction: TxType::Legacy,
            receipt: TxType::Eip1559,
        }
    );
}

#[test]
fn block_conversion_rejects_pre_eip658_receipts() {
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![address(1)],
        21_000,
        30_000,
    );

    let stored_receipts = [stored_receipt(TxType::Legacy, true, 21_000, Vec::new())];

    let error = receipts_from_block(&block, &stored_receipts, MAINNET.as_ref()).unwrap_err();

    assert_eq!(error, ReceiptConstructionError::PreEip658Block);
}

#[test]
fn block_conversion_rejects_final_gas_mismatch() {
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![address(1)],
        22_000,
        30_000,
    );

    let stored_receipts = [stored_receipt(TxType::Legacy, true, 21_000, Vec::new())];

    let error = receipts_from_block(&block, &stored_receipts, &chain_spec()).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::BlockGasUsedMismatch { header: 22_000, receipts: 21_000 }
    );
}

#[test]
fn failed_set_code_status_keeps_recovered_authorities() {
    let (block, stored) = set_code_block_fixture(false);

    let first = Address::from(hex!("4fd357b597c2d9c930a24645958f8cbc43a11d2e"));
    let second = Address::from(hex!("fc8ceb2413f8f3808eab57499da4a8b5179f6820"));

    let snapshot = Eip6466ReceiptSnapshot::from_block(&block, &stored, &chain_spec()).unwrap();

    let Receipt::SetCode(receipt) = snapshot.receipts().get(0).unwrap() else {
        panic!("expected SetCode receipt");
    };

    assert!(!receipt.status);
    assert_eq!(receipt.authorities, vec![first, second]);
    assert_eq!(snapshot.root(), snapshot.tree().root());

    assert_eq!(
        snapshot.serialized().as_ref(),
        hex!(
            "0400000003\
             6161616161616161616161616161616161616161\
             3075000000000000\
             250000000025000000\
             4fd357b597c2d9c930a24645958f8cbc43a11d2e\
             fc8ceb2413f8f3808eab57499da4a8b5179f6820"
        ),
    );

    assert_eq!(
        snapshot.root(),
        b256!("c8decef7337bea373ee787f2988db6490da7539d6046a1b9b8116352e7cd57cf"),
    );
}

fn minimal_basic_receipt() -> Receipt {
    Receipt::Basic(BasicReceipt {
        from_: address(0x11),
        gas_used: 21_000,
        logs: Vec::new(),
        status: true,
    })
}

#[test]
fn minimal_basic_matches_pinned_ssz_specs_reference() {
    let receipt = minimal_basic_receipt();
    let container = match &receipt {
        Receipt::Basic(receipt) => {
            let length = checked_basic_receipt_length(receipt).unwrap();
            let mut bytes = Vec::with_capacity(length);
            append_basic_receipt(receipt, &mut bytes);
            bytes
        }
        _ => unreachable!(),
    };
    let union = receipt.to_ssz_bytes().unwrap();
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![receipt])).unwrap();

    assert_eq!(
        container,
        hex!(
            "1111111111111111111111111111111111111111\
                0852000000000000\
                21000000\
                01"
        )
    );
    assert_eq!(
        union,
        hex!(
            "01\
                1111111111111111111111111111111111111111\
                0852000000000000\
                21000000\
                01"
        )
    );
    assert_eq!(
        snapshot.serialized().as_ref(),
        hex!(
            "04000000\
                01\
                1111111111111111111111111111111111111111\
                0852000000000000\
                21000000\
                01"
        )
    );

    let receipt = snapshot.receipts().get(0).unwrap();

    assert_eq!(
        build_receipt_container_tree(receipt).unwrap().root(),
        b256!("651fec86de7d6062fe59e2f8d78d713e62b4b2f9a126961db71c6c01bbe22b35")
    );
    assert_eq!(
        build_receipt_tree(receipt).unwrap().root(),
        b256!("278722ac6eec06c3343d693e6b6610273496810319affbd452879e4092690585")
    );
    assert_eq!(
        snapshot.root(),
        b256!("0ac23fa89d23b2ef1f5b42b1e90b6949d85b68cbbb91a15f41a33b5a8b524a65")
    );
}

#[test]
fn snapshot_retains_selector_and_active_fields() {
    let snapshot =
        Eip6466ReceiptSnapshot::build(Receipts::new(vec![minimal_basic_receipt()])).unwrap();

    let list_contents = &snapshot.tree().children().unwrap()[0];
    let union = &list_contents.children().unwrap()[0];
    let union_children = union.children().unwrap();
    let container_children = union_children[0].children().unwrap();

    let mut selector = [0_u8; 32];
    selector[0] = BASIC_RECEIPT_SELECTOR;
    assert_eq!(union_children[1].root(), B256::from(selector));

    let mut active_fields = [0_u8; 32];
    active_fields[0] = 0x1b;
    assert_eq!(container_children[1].root(), B256::from(active_fields));

    assert_eq!(snapshot.root(), snapshot.tree().root());
}

#[test]
fn empty_receipt_list_serializes_without_an_offset() {
    let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(Vec::new())).unwrap();

    assert!(snapshot.serialized().is_empty());
    assert!(snapshot.receipts().is_empty());
}

#[test]
fn create_and_set_code_use_pinned_wire_layouts() {
    let create = Receipt::Create(CreateReceipt {
        from_: address(0x11),
        gas_used: 21_000,
        contract_address: address(0x22),
        logs: Vec::new(),
        status: true,
    });
    let set_code = Receipt::SetCode(SetCodeReceipt {
        from_: address(0x11),
        gas_used: 21_000,
        logs: Vec::new(),
        status: false,
        authorities: vec![address(0x22), Address::ZERO],
    });

    assert_eq!(
        create.to_ssz_bytes().unwrap(),
        hex!(
            "02\
                1111111111111111111111111111111111111111\
                0852000000000000\
                2222222222222222222222222222222222222222\
                35000000\
                01"
        )
    );
    assert_eq!(
        set_code.to_ssz_bytes().unwrap(),
        hex!(
            "03\
                1111111111111111111111111111111111111111\
                0852000000000000\
                25000000\
                00\
                25000000\
                2222222222222222222222222222222222222222\
                0000000000000000000000000000000000000000"
        )
    );
}

#[test]
fn nested_variable_offsets_are_exact() {
    let log =
        Log::new(address(0x11), vec![B256::repeat_byte(0x22)], Bytes::from(vec![0xaa, 0xbb, 0xcc]))
            .unwrap();
    let length = checked_log_length(&log).unwrap();
    let mut encoded_log = Vec::with_capacity(length);
    append_log(&log, &mut encoded_log);

    assert_eq!(
        encoded_log,
        hex!(
            "1111111111111111111111111111111111111111\
                1c000000\
                3c000000\
                2222222222222222222222222222222222222222222222222222222222222222\
                aabbcc"
        )
    );

    let receipt = Receipt::Basic(BasicReceipt {
        from_: address(0x33),
        gas_used: 1,
        logs: vec![log],
        status: true,
    });
    let encoded_receipt = receipt.to_ssz_bytes().unwrap();

    assert_eq!(&encoded_receipt[29..33], &[0x21, 0, 0, 0]);
    assert_eq!(&encoded_receipt[34..38], &[4, 0, 0, 0]);
}

#[test]
fn serialization_length_arithmetic_is_checked() {
    assert_eq!(
        checked_add_length("test value", usize::MAX, 1),
        Err(ReceiptSerializationError::ArithmeticOverflow { context: "test value" })
    );
    assert_eq!(
        checked_mul_length("test value", usize::MAX, 2),
        Err(ReceiptSerializationError::ArithmeticOverflow { context: "test value" })
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn serialization_rejects_values_above_the_ssz_offset_limit() {
    let length = MAX_LENGTH_VALUE + 1;

    assert_eq!(
        checked_serialized_length("test value", length),
        Err(ReceiptSerializationError::MaximumLengthExceeded { context: "test value", length })
    );
}

#[test]
fn snapshot_distinguishes_missing_data_from_conversion_failure() {
    let empty = recovered_block(Vec::new(), Vec::new(), 0, 0);

    assert!(matches!(
        Eip6466ReceiptSnapshot::from_block(&empty, &[], MAINNET.as_ref()),
        Err(Eip6466SnapshotError::MissingData(ReceiptConstructionError::PreEip658Block))
    ));

    let missing_receipt = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![Address::ZERO],
        0,
        0,
    );

    assert!(matches!(
        Eip6466ReceiptSnapshot::from_block(&missing_receipt, &[], &chain_spec()),
        Err(Eip6466SnapshotError::MissingData(ReceiptConstructionError::InputCountMismatch { .. }))
    ));

    let invalid_header = recovered_block(Vec::new(), Vec::new(), 1, 0);

    assert!(matches!(
        Eip6466ReceiptSnapshot::from_block(&invalid_header, &[], &chain_spec()),
        Err(Eip6466SnapshotError::Conversion(
            ReceiptConstructionError::HeaderGasUsedExceedsLimit { .. }
        ))
    ));
}

#[test]
fn duplicate_authorization_tuples_keep_separate_recovered_positions() {
    let mut payload = set_code_transaction();
    payload.authorization_list.insert(1, payload.authorization_list[0].clone());
    let transaction = TransactionSigned::new_unhashed(
        EthereumTransaction::Eip7702(payload),
        Signature::test_signature(),
    );
    let receipt =
        receipt_from_transaction(&transaction, address(1), 30_000, Vec::new(), true).unwrap();
    let first = Address::from(hex!("4fd357b597c2d9c930a24645958f8cbc43a11d2e"));
    let second = Address::from(hex!("fc8ceb2413f8f3808eab57499da4a8b5179f6820"));
    assert_eq!(receipt.authorities().unwrap(), &[first, first, second]);
}

#[test]
fn mixed_block_recovers_authorities_from_its_own_ordered_transactions() {
    let set_code = TransactionSigned::new_unhashed(
        EthereumTransaction::Eip7702(set_code_transaction()),
        Signature::test_signature(),
    );
    let block = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO)), set_code],
        vec![address(1), address(2)],
        51_000,
        100_000,
    );
    let stored = [
        stored_receipt(TxType::Legacy, true, 21_000, Vec::new()),
        stored_receipt(TxType::Eip7702, true, 51_000, Vec::new()),
    ];
    let converted = receipts_from_block(&block, &stored, &chain_spec()).unwrap();
    assert_eq!(converted.len(), 2);
    assert_eq!(converted.get(0).unwrap().from_(), address(1));
    assert_eq!(converted.get(0).unwrap().gas_used(), 21_000);
    assert_eq!(converted.get(1).unwrap().from_(), address(2));
    assert_eq!(converted.get(1).unwrap().gas_used(), 30_000);
    assert_eq!(
        converted.get(1).unwrap().authorities().unwrap(),
        &[
            Address::from(hex!("4fd357b597c2d9c930a24645958f8cbc43a11d2e")),
            Address::from(hex!("fc8ceb2413f8f3808eab57499da4a8b5179f6820")),
        ]
    );
}

#[test]
fn log_data_and_topic_boundaries_match_independent_receipt_roots() {
    for (length, topic_count, expected) in [
        (0, 2, b256!("c799d094f7dacafac5165d4dcdf7f4b2d578ceb654f593e18e3e8ec8a5d1420d")),
        (1, 2, b256!("0ac618a508f445c0ef258f5c6988254b0b4e4428253365425c805567abae7eb9")),
        (31, 2, b256!("81b207589c5402fb300a47e8f27b9e561e36ae93e6b3c701957912255e610aac")),
        (32, 2, b256!("c45ddf0c1d844655a01316aa88287600fda1e2aae79c4c8d35b816273004d91c")),
        (33, 2, b256!("3f8fe05d8f7d17aa4044bc1394dc6085e4672c8ad784b8cebc8c18f52c15ece2")),
        (160, 2, b256!("4cd08a680012f384afb634e61e674fca24ba58499b27d49b51c51705ad07be91")),
        (161, 2, b256!("99a7b2250be5c97b722fdc76399613fd0c2f69f260650d7265d49ab144425ce0")),
        (3, 0, b256!("47e01cb1306c628bd4553c0beaab7d13efc32b9ef0c706849a31674dc701dbdb")),
        (3, 4, b256!("d611ecece864519dbd21df88ce2c743508f09f43eb8795832ce04bcce76f220b")),
    ] {
        let receipt = Receipt::Basic(BasicReceipt {
            from_: address(0x11),
            gas_used: 21_000,
            logs: vec![Log::new(
                address(0x44),
                (0..topic_count).map(|index| B256::repeat_byte(0x55 + index)).collect(),
                Bytes::from(vec![0xab; length]),
            )
            .unwrap()],
            status: true,
        });
        let snapshot = Eip6466ReceiptSnapshot::build(Receipts::new(vec![receipt])).unwrap();
        assert_eq!(snapshot.root(), expected, "data length {length}, topic count {topic_count}");
    }
}
