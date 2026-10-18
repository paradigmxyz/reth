use super::*;
use alloy_consensus::{SignableTransaction, TxLegacy};
use reth_db_api::{
    models::StoredBlockOmmers,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{FullBlock, FullBlockBody, FullBlockHeader};
use reth_provider::{
    test_utils::create_test_provider_factory, DBProvider, DatabaseProviderFactory,
};
use ssz::Encode;

fn fixture_summary() -> ExecutionSummary {
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/payload.json")).unwrap();
    let bytes =
        alloy_primitives::hex::decode(reference["cases"][0]["summary"].as_str().unwrap()).unwrap();
    let mut summary = ExecutionSummary::from_ssz_bytes(&bytes).unwrap();
    summary.base_fees_per_gas.regular = U256::from(7);
    summary.base_fees_per_gas.blob = U256::from(1);
    summary
}

#[test]
fn custom_primitives_satisfy_the_enabled_native_and_compact_contracts() {
    fn primitives<N: NodePrimitives>() {}
    fn header<H: FullBlockHeader + Compact>() {}
    fn body<B: FullBlockBody>() {}
    fn block<B: FullBlock>() {}
    primitives::<ExecutionPrimitives>();
    header::<ExecutionHeader>();
    body::<ExecutionBody>();
    block::<ExecutionBlock>();
}

#[test]
fn header_emptiness_uses_the_selected_commitment_and_survives_readback() {
    assert_eq!(EMPTY_SSZ_LIST_ROOT, TransactionSnapshot::build(Vec::new()).unwrap().root());
    assert_eq!(EMPTY_SSZ_LIST_ROOT, WithdrawalSnapshot::build(Vec::new()).unwrap().root());
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/payload.json")).unwrap();
    let transaction_root = reference["cases"][1]["component_nodes"]["transactions"]
        .as_str()
        .unwrap()
        .parse::<B256>()
        .unwrap();
    let withdrawal_root =
        WithdrawalSnapshot::build(vec![alloy_eips::eip4895::Withdrawal::default()]).unwrap().root();
    assert_ne!(transaction_root, EMPTY_SSZ_LIST_ROOT);
    assert_ne!(withdrawal_root, EMPTY_SSZ_LIST_ROOT);
    for transactions_empty in [false, true] {
        for withdrawals_empty in [false, true] {
            let mut summary = fixture_summary();
            summary.transactions_root =
                if transactions_empty { EMPTY_SSZ_LIST_ROOT } else { transaction_root };
            summary.withdrawals_root =
                if withdrawals_empty { EMPTY_SSZ_LIST_ROOT } else { withdrawal_root };
            let header = ExecutionHeader::from_summary(summary).unwrap();
            let mut compact = Vec::new();
            header.to_compact(&mut compact);
            for header in [
                header.clone(),
                ExecutionHeader::decode(&mut alloy_rlp::encode(&header).as_slice()).unwrap(),
                ExecutionHeader::decompress(&compact).unwrap(),
                serde_json::from_value(serde_json::to_value(&header).unwrap()).unwrap(),
            ] {
                let expected = transactions_empty && withdrawals_empty;
                assert_eq!(header.is_empty(), expected);
                assert_eq!(
                    reth_primitives_traits::SealedHeader::seal_slow(header).is_empty(),
                    expected
                );
            }
        }
    }
    for native in [
        Header::default(),
        Header { transactions_root: transaction_root, ..Default::default() },
        Header { withdrawals_root: Some(withdrawal_root), ..Default::default() },
        Header { ommers_hash: B256::ZERO, ..Default::default() },
    ] {
        assert_eq!(ExecutionHeader::legacy(native.clone()).is_empty(), native.is_empty());
    }
}

#[test]
fn native_blob_fee_preserves_the_summary_and_legacy_calculation_after_readback() {
    let params = BlobParams::cancun();
    for (fee, excess) in
        [(0, 0), (1337, 0), (u128::MAX, 0), (params.calc_blob_fee(10_000_000), 10_000_000)]
    {
        let mut summary = fixture_summary();
        summary.base_fees_per_gas.blob = U256::from(fee);
        summary.excess_gas.blob = excess;
        let header = ExecutionHeader::from_summary(summary).unwrap();
        let hash = header.hash_slow();
        let mut compact = Vec::new();
        header.to_compact(&mut compact);
        for header in [
            header.clone(),
            ExecutionHeader::decode(&mut alloy_rlp::encode(&header).as_slice()).unwrap(),
            ExecutionHeader::decompress(&compact).unwrap(),
            serde_json::from_value(serde_json::to_value(&header).unwrap()).unwrap(),
        ] {
            assert_eq!(header.blob_fee(params), Some(fee));
            assert_eq!(header.hash_slow(), hash);
            let sealed = reth_primitives_traits::SealedHeader::seal_slow(header);
            assert_eq!(sealed.blob_fee(params), Some(fee));
            assert_eq!(sealed.hash(), hash);
        }
    }
    for excess in [None, Some(0), Some(10_000_000)] {
        let native = Header { excess_blob_gas: excess, ..Default::default() };
        let header = ExecutionHeader::legacy(native.clone());
        assert_eq!(header.blob_fee(params), native.blob_fee(params));
    }
}

#[test]
fn legacy_compact_flag_widths_cannot_collide_with_the_ssz_prefix() {
    for number in [0, 1, 255, 256, u16::MAX as u64, u32::MAX as u64, u64::MAX] {
        let native = Header {
            difficulty: U256::MAX,
            number,
            gas_limit: number,
            gas_used: number,
            timestamp: number,
            nonce: B64::repeat_byte(0xff),
            logs_bloom: Bloom::repeat_byte(0xff),
            base_fee_per_gas: Some(number),
            blob_gas_used: Some(number),
            excess_blob_gas: Some(number),
            withdrawals_root: Some(B256::repeat_byte(0xff)),
            parent_beacon_block_root: Some(B256::repeat_byte(0xff)),
            requests_hash: Some(B256::repeat_byte(0xff)),
            block_access_list_hash: Some(B256::repeat_byte(0xff)),
            slot_number: Some(number),
            extra_data: Bytes::from(vec![0xff; 32]),
            ..Default::default()
        };
        let mut encoded = Vec::new();
        native.to_compact(&mut encoded);
        assert!(!encoded.starts_with(&SSZ_HEADER_PREFIX));
        assert_eq!(ExecutionHeader::decompress(&encoded).unwrap(), ExecutionHeader::legacy(native));
    }
}

#[test]
fn legacy_header_and_block_wire_bytes_and_identity_remain_native() {
    let transaction: TransactionSigned = TxLegacy {
        chain_id: Some(1),
        nonce: 3,
        gas_limit: 21_000,
        gas_price: 7,
        to: alloy_primitives::TxKind::Call(Address::repeat_byte(0x44)),
        ..Default::default()
    }
    .into_signed(alloy_primitives::Signature::new(U256::from(1), U256::from(2), false))
    .into();
    for withdrawals in [None, Some(vec![alloy_eips::eip4895::Withdrawal::default()].into())] {
        let body = alloy_consensus::BlockBody {
            transactions: vec![transaction.clone()],
            ommers: if withdrawals.is_none() {
                vec![Header { number: 16, ..Default::default() }]
            } else {
                Vec::new()
            },
            withdrawals,
        };
        let native = Header {
            number: 17,
            timestamp: 123,
            parent_hash: B256::repeat_byte(0x11),
            transactions_root: body.calculate_tx_root(),
            withdrawals_root: body.calculate_withdrawals_root(),
            ommers_hash: body.calculate_ommers_root(),
            base_fee_per_gas: body.withdrawals.as_ref().map(|_| 7),
            ..Default::default()
        };
        let header = ExecutionHeader::legacy(native.clone());
        let mut compact = Vec::new();
        let mut native_compact = Vec::new();
        header.to_compact(&mut compact);
        native.to_compact(&mut native_compact);
        assert_eq!(compact, native_compact);
        assert!(!compact.starts_with(&SSZ_HEADER_PREFIX));
        assert_eq!(ExecutionHeader::from_compact(&native_compact, native_compact.len()).0, header);
        assert_eq!(header.hash_slow(), native.hash_slow());
        assert_eq!(alloy_rlp::encode(&header), alloy_rlp::encode(&native));
        let wrapped = EthereumBody {
            transactions: body.transactions.clone(),
            ommers: body.ommers.iter().cloned().map(ExecutionHeader::legacy).collect(),
            withdrawals: body.withdrawals.clone(),
        };
        let wrapped = ExecutionBody::from_header(&header, wrapped).unwrap();
        assert_eq!(alloy_rlp::encode(&wrapped), alloy_rlp::encode(&body));
        let block = ExecutionBlock::try_new(header, wrapped).unwrap();
        let native = alloy_consensus::Block { header: native, body };
        let bytes = alloy_rlp::encode(&block);
        assert_eq!(bytes, alloy_rlp::encode(&native));
        assert_eq!(
            alloy_consensus::Block::<TransactionSigned>::decode(&mut bytes.as_slice()).unwrap(),
            native,
        );
        assert_eq!(ExecutionBlock::decode(&mut bytes.as_slice()).unwrap(), block);
    }
}

#[test]
fn summary_identity_survives_rlp_compact_and_serde_without_trusting_a_cached_hash() {
    let summary = fixture_summary();
    let expected = summary.tree().unwrap().root();
    let header = ExecutionHeader::from_summary(summary.clone()).unwrap();
    assert!(header.is_ssz());
    assert_eq!(header.hash_slow(), expected);
    assert_eq!(header.logs_bloom(), Bloom::repeat_byte(0xff));
    assert_eq!(header.requests_hash(), Some(summary.requests_root));
    assert_eq!(header.block_access_list_hash(), Some(summary.block_access_list_root));
    for header in [header, ExecutionHeader::legacy(Header::default())] {
        let bytes = alloy_rlp::encode(&header);
        let mut input = bytes.as_slice();
        assert_eq!(ExecutionHeader::decode(&mut input).unwrap(), header);
        assert!(input.is_empty());
        let mut compact = Vec::new();
        let len = header.to_compact(&mut compact);
        assert_eq!(len, compact.len());
        compact.extend_from_slice(&[7, 8]);
        let (decoded, rest) = ExecutionHeader::from_compact(&compact, len);
        assert_eq!(decoded, header);
        assert_eq!(rest, &[7, 8]);
        let value = serde_json::to_value(&header).unwrap();
        assert_eq!(serde_json::from_value::<ExecutionHeader>(value).unwrap(), header);
    }
    let mut invalid = summary;
    invalid.gas_used.regular = invalid.gas_limits.regular + 1;
    let value = serde_json::to_value(HeaderEncoding::Ssz(invalid.as_ssz_bytes().into())).unwrap();
    assert!(serde_json::from_value::<ExecutionHeader>(value).is_err());
    let invalid = Bytes::from(invalid.as_ssz_bytes());
    assert!(ExecutionHeader::decode(&mut alloy_rlp::encode(&invalid).as_slice()).is_err());
    let mut stored = SSZ_HEADER_PREFIX.to_vec();
    stored.extend_from_slice(&invalid);
    assert!(ExecutionHeader::decompress(&stored).is_err());
    assert!(ExecutionHeader::decompress(&SSZ_HEADER_PREFIX).is_err());
    assert!(ExecutionHeader::decompress(&[]).is_err());
    for regular in [true, false] {
        let mut summary = fixture_summary();
        if regular {
            summary.base_fees_per_gas.regular = U256::from(u64::MAX) + U256::from(1);
        } else {
            summary.base_fees_per_gas.blob = U256::from(u128::MAX) + U256::from(1);
        }
        assert!(matches!(
            ExecutionHeader::from_summary(summary),
            Err(PrimitiveError::UnsupportedFeeRange)
        ));
    }
}

#[test]
fn body_root_dispatch_and_codecs_reject_mismatched_or_missing_components() {
    let mut summary = fixture_summary();
    let transactions = TransactionSnapshot::build(Vec::new()).unwrap();
    let withdrawals = WithdrawalSnapshot::build(Vec::new()).unwrap();
    summary.transactions_root = transactions.root();
    summary.withdrawals_root = withdrawals.root();
    let header = ExecutionHeader::from_summary(summary).unwrap();
    assert!(ExecutionBody::from_header(&header, EthereumBody::default()).is_err());
    assert!(ExecutionBody::from_header(
        &header,
        EthereumBody {
            ommers: vec![ExecutionHeader::default()],
            withdrawals: Some(Default::default()),
            ..Default::default()
        },
    )
    .is_err());
    assert!(ExecutionBody::from_header(
        &ExecutionHeader::default(),
        EthereumBody { ommers: vec![header.clone()], ..Default::default() },
    )
    .is_err());
    let body = ExecutionBody::from_header(
        &header,
        EthereumBody { withdrawals: Some(Default::default()), ..Default::default() },
    )
    .unwrap();
    assert_eq!(body.calculate_tx_root(), transactions.root());
    assert_eq!(body.calculate_withdrawals_root(), Some(withdrawals.root()));
    assert!(body.calculate_ommers_root().is_none());
    let bytes = alloy_rlp::encode(&body);
    assert_eq!(bytes.len(), body.length());
    assert_eq!(ExecutionBody::decode(&mut bytes.as_slice()).unwrap(), body);
    let value = serde_json::to_value(&body).unwrap();
    assert_eq!(serde_json::from_value::<ExecutionBody>(value).unwrap(), body);
    let block = ExecutionBlock::try_new(header.clone(), body.clone()).unwrap();
    let bytes = alloy_rlp::encode(&block);
    assert_eq!(bytes.len(), block.length());
    assert_eq!(ExecutionBlock::decode(&mut bytes.as_slice()).unwrap(), block);
    assert!(ExecutionBlock::try_new(ExecutionHeader::default(), body).is_err());
    let wrong = ExecutionBody::from_header(
        &header,
        EthereumBody {
            withdrawals: Some(vec![alloy_eips::eip4895::Withdrawal::default()].into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(ExecutionBlock::try_new(header.clone(), wrong.clone()).is_err());
    let invalid = EthereumBlock { header, body: wrong.into_ethereum_body() };
    assert!(ExecutionBlock::decode(&mut alloy_rlp::encode(&invalid).as_slice()).is_err());
    assert!(
        serde_json::from_value::<ExecutionBlock>(serde_json::to_value(&invalid).unwrap()).is_err()
    );
}

#[test]
fn stored_headers_and_indices_round_trip_through_native_tables() {
    let factory = create_test_provider_factory();
    for header in [
        ExecutionHeader::legacy(Header { number: 1, ..Default::default() }),
        ExecutionHeader::from_summary(fixture_summary()).unwrap(),
    ] {
        let number = header.number();
        let hash = header.hash_slow();
        let writer = factory.database_provider_rw().unwrap();
        writer.tx_ref().put::<tables::Headers<ExecutionHeader>>(number, header.clone()).unwrap();
        writer.tx_ref().put::<tables::HeaderNumbers>(hash, number).unwrap();
        writer.tx_ref().put::<tables::CanonicalHeaders>(number, hash).unwrap();
        writer.commit().unwrap();
        let reader = factory.database_provider_ro().unwrap();
        let stored =
            reader.tx_ref().get::<tables::Headers<ExecutionHeader>>(number).unwrap().unwrap();
        assert_eq!(stored, header);
        assert_eq!(stored.hash_slow(), hash);
        assert_eq!(reader.tx_ref().get::<tables::HeaderNumbers>(hash).unwrap(), Some(number));
        assert_eq!(reader.tx_ref().get::<tables::CanonicalHeaders>(number).unwrap(), Some(hash));
    }
    let ommers = vec![Header::default(), Header { number: 1, ..Default::default() }];
    let native = StoredBlockOmmers { ommers: ommers.clone() };
    let wrapped = StoredBlockOmmers {
        ommers: ommers.into_iter().map(ExecutionHeader::legacy).collect::<Vec<_>>(),
    };
    assert_eq!(wrapped.clone().compress(), native.compress());
    let writer = factory.database_provider_rw().unwrap();
    writer.tx_ref().put::<tables::BlockOmmers<ExecutionHeader>>(1, wrapped.clone()).unwrap();
    writer.commit().unwrap();
    let reader = factory.database_provider_ro().unwrap();
    assert_eq!(
        reader.tx_ref().get::<tables::BlockOmmers<ExecutionHeader>>(1).unwrap(),
        Some(wrapped),
    );
}
