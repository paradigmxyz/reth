use super::*;
use alloy_consensus::{Header, TxLegacy};
use alloy_primitives::{b256, hex, Log as ExecutionLog, Signature, TxKind};
use reth_ethereum_primitives::{BlockBody, Transaction as EthereumTransaction};

fn address(byte: u8) -> Address {
    Address::from([byte; 20])
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
        EthereumTransaction::Legacy(TxLegacy { nonce, to, ..Default::default() }),
        Signature::test_signature(),
    )
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
fn controlled_authorization_outcomes_preserve_order() {
    let first = address(1);
    let third = address(3);
    let outcomes = [
        AuthorizationOutcome::Success(first),
        AuthorizationOutcome::Failure,
        AuthorizationOutcome::Success(third),
    ];

    assert_eq!(
        authorization_addresses(outcomes.len(), Some(&outcomes)).unwrap(),
        vec![first, Address::ZERO, third]
    );
}

#[test]
fn missing_or_incomplete_authorization_outcomes_are_rejected() {
    assert_eq!(
        authorization_addresses(1, None),
        Err(ReceiptConstructionError::MissingAuthorizationOutcomes)
    );

    let outcomes = [AuthorizationOutcome::Success(address(1))];
    assert_eq!(
        authorization_addresses(2, Some(&outcomes)),
        Err(ReceiptConstructionError::AuthorizationOutcomeCountMismatch { expected: 2, actual: 1 })
    );
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

    let authorization_outcomes = [None, None];

    let converted =
        receipts_from_block(&block, &stored_receipts, &authorization_outcomes, true).unwrap();

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
fn failed_create_preserves_transaction_candidate_address() {
    let sender = address(0x41);
    let nonce = 7;

    let block = recovered_block(
        vec![legacy_transaction(nonce, TxKind::Create)],
        vec![sender],
        53_000,
        60_000,
    );

    let stored_receipts = [stored_receipt(TxType::Legacy, false, 53_000, Vec::new())];

    let converted = receipts_from_block(&block, &stored_receipts, &[None], true).unwrap();

    match converted.get(0).unwrap() {
        Receipt::Create(receipt) => {
            assert_eq!(receipt.from_, sender);
            assert_eq!(receipt.contract_address, sender.create(nonce));
            assert_eq!(receipt.gas_used, 53_000);
            assert!(!receipt.status);
        }
        receipt => panic!("expected Create receipt, got {receipt:?}"),
    }
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

    let converted = receipts_from_block(&block, &stored_receipts, &[None, None], true).unwrap();

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

    let error = receipts_from_block(&block, &stored_receipts, &[None, None], true).unwrap_err();

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

    let stored_receipts = [stored_receipt(TxType::Legacy, true, 21_000, Vec::new())];

    let error = receipts_from_block(&block, &stored_receipts, &[], true).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::InputCountMismatch {
            transactions: 1,
            receipts: 1,
            authorization_outcomes: 0,
        }
    );
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

    let error = receipts_from_block(&block, &stored_receipts, &[None], true).unwrap_err();

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

    let error = receipts_from_block(&block, &stored_receipts, &[None], false).unwrap_err();

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

    let error = receipts_from_block(&block, &stored_receipts, &[None], true).unwrap_err();

    assert_eq!(
        error,
        ReceiptConstructionError::BlockGasUsedMismatch { header: 22_000, receipts: 21_000 }
    );
}

#[test]
fn failed_set_code_status_keeps_successful_authorities() {
    let first = address(0x51);
    let third = address(0x53);
    let outcomes = [
        AuthorizationOutcome::Success(first),
        AuthorizationOutcome::Failure,
        AuthorizationOutcome::Success(third),
    ];

    let authorities = authorization_addresses(outcomes.len(), Some(&outcomes)).unwrap();

    let receipt = SetCodeReceipt {
        from_: address(0x61),
        gas_used: 30_000,
        logs: Vec::new(),
        status: false,
        authorities,
    };

    assert!(!receipt.status);
    assert_eq!(receipt.authorities, vec![first, Address::ZERO, third]);
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
        Eip6466ReceiptSnapshot::from_block(&empty, &[], &[], false),
        Err(Eip6466SnapshotError::MissingData(ReceiptConstructionError::PreEip658Block))
    ));

    let missing_receipt = recovered_block(
        vec![legacy_transaction(0, TxKind::Call(Address::ZERO))],
        vec![Address::ZERO],
        0,
        0,
    );

    assert!(matches!(
        Eip6466ReceiptSnapshot::from_block(&missing_receipt, &[], &[None], true,),
        Err(Eip6466SnapshotError::MissingData(ReceiptConstructionError::InputCountMismatch { .. }))
    ));

    let invalid_header = recovered_block(Vec::new(), Vec::new(), 1, 0);

    assert!(matches!(
        Eip6466ReceiptSnapshot::from_block(&invalid_header, &[], &[], true,),
        Err(Eip6466SnapshotError::Conversion(
            ReceiptConstructionError::HeaderGasUsedExceedsLimit { .. }
        ))
    ));
}
