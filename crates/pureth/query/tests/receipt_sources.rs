#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![cfg(not(target_arch = "wasm32"))]

use alloy_consensus::{
    transaction::Recovered, ReceiptEnvelope, SignableTransaction, Transaction, TxEip7702, TxLegacy,
};
use alloy_evm::block::BlockExecutor;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use jsonrpsee::core::server::Methods;
use reth_chainspec::ChainSpecBuilder;
use reth_db_api::{tables, transaction::DbTxMut};
use reth_ethereum_primitives::{Block, BlockBody, TransactionSigned};
use reth_evm::ConfigureEvm;
use reth_evm_ethereum::{factory::RethEvmFactory, EthEvmConfig};
use reth_primitives_traits::{Block as _, RecoveredBlock};
use reth_provider::{
    providers::BlockchainProvider,
    test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
    BlockWriter, DBProvider, DatabaseProviderFactory, ExecutionOutcome, OriginalValuesKnown,
    StateWriteConfig, StateWriter,
};
use reth_pureth_query::{
    convert_eip6466_block_receipts, convert_eip6466_receipt, convert_eip6466_receipt_from_retained,
    extend_block_receipt_responses, extend_receipt_response, query_snapshot,
    verify_eip6466_receipt_response, verify_query_response, FilterOutput, FilterPredicate,
    PurethApiServer, PurethRpc, QueryError, QueryHandler, QueryRequest, QueryResponse,
    QueryService, ReceiptProofSupport, ReceiptResponse, ReceiptSelection, SelectionError,
    SelectionLimits, SelectionOperation, SelectionRequest,
};
use reth_pureth_receipt::{HistoricalAcquisitionError, ProviderSnapshot};
use reth_rpc_convert::{
    transaction::{ConvertReceiptInput, ReceiptConverter},
    RpcConverter,
};
use reth_rpc_eth_types::{
    logs_utils::{append_matching_block_logs, matching_block_logs_with_tx_hashes, ProviderOrBlock},
    receipt::EthReceiptConverter,
};
use revm::{
    bytecode::Bytecode,
    context_interface::transaction::Authorization,
    database::{states::bundle_state::BundleRetention, CacheDB, EmptyDB, State},
    state::AccountInfo,
};
use std::sync::{atomic::AtomicBool, Arc};

fn fixture(
) -> (BlockchainProvider<MockNodeTypesWithDB>, RecoveredBlock<Block>, ExecutionOutcome, Address) {
    fixture_with_additional_authorizations(0)
}

fn fixture_with_additional_authorizations(
    additional: u64,
) -> (BlockchainProvider<MockNodeTypesWithDB>, RecoveredBlock<Block>, ExecutionOutcome, Address) {
    let sender = PrivateKeySigner::from_bytes(&B256::repeat_byte(9)).unwrap();
    let authority = PrivateKeySigner::from_bytes(&B256::repeat_byte(1)).unwrap();
    let auth = |chain, nonce| {
        let auth = Authorization {
            chain_id: U256::from(chain),
            address: Address::repeat_byte(0x44),
            nonce,
        };
        auth.clone().into_signed(authority.sign_hash_sync(&auth.signature_hash()).unwrap())
    };
    let mut database = CacheDB::<EmptyDB>::default();
    database.insert_account_info(
        sender.address(),
        AccountInfo { balance: U256::from(10u64.pow(18)), ..Default::default() },
    );
    for (address, bytes) in [
        (Address::repeat_byte(0xbb), "6001600052600160206000a100"),
        (Address::repeat_byte(0xcc), "60006000fd"),
    ] {
        let code = Bytecode::new_raw(Bytes::from(alloy_primitives::hex::decode(bytes).unwrap()));
        database.insert_account_info(
            address,
            AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
        );
    }
    let mut transactions = Vec::<TransactionSigned>::new();
    for (nonce, to, authorization_list) in [
        (
            0,
            Address::repeat_byte(0xbb),
            [
                auth(2u64, 0),
                auth(1, 0),
                auth(1, 1),
                Authorization {
                    chain_id: U256::from(1),
                    address: Address::repeat_byte(0x44),
                    nonce: 2,
                }
                .into_signed(alloy_primitives::Signature::new(
                    U256::ZERO,
                    U256::from(1),
                    false,
                )),
            ]
            .into_iter()
            .chain((2..2 + additional).map(|nonce| auth(1, nonce)))
            .collect(),
        ),
        (
            1,
            Address::repeat_byte(0xcc),
            [auth(1, 99 + 2 * additional), auth(0, 2 + additional)]
                .into_iter()
                .chain((3 + additional..3 + 2 * additional).map(|nonce| auth(0, nonce)))
                .collect(),
        ),
    ] {
        let tx = TxEip7702 {
            chain_id: 1,
            nonce,
            gas_limit: 250_000 + additional * 25_000,
            to,
            authorization_list,
            ..Default::default()
        };
        transactions.push(
            tx.clone().into_signed(sender.sign_hash_sync(&tx.signature_hash()).unwrap()).into(),
        );
    }
    for (nonce, to, input) in [
        (2, TxKind::Create, Bytes::from_static(&[0x60, 0, 0x60, 0, 0xfd])),
        (3, TxKind::Call(Address::repeat_byte(0xbb)), Bytes::new()),
        (4, TxKind::Create, Bytes::from_static(&[0x60, 1, 0x60, 0, 0xf3])),
    ] {
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_limit: 100_000,
            to,
            input,
            ..Default::default()
        };
        transactions.push(
            tx.clone().into_signed(sender.sign_hash_sync(&tx.signature_hash()).unwrap()).into(),
        );
    }
    let mut block = Block {
        header: alloy_consensus::Header {
            timestamp: 1_747_000_000,
            gas_limit: 2_000_000 + additional * 50_000,
            base_fee_per_gas: Some(0),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: Some(B256::ZERO),
            ..Default::default()
        },
        body: BlockBody {
            transactions,
            withdrawals: Some(Default::default()),
            ..Default::default()
        },
    };
    let chain = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
    let config = EthEvmConfig::new_with_evm_factory(chain.clone(), RethEvmFactory::default());
    let sealed = block.clone().seal_slow();
    let mut state = State::builder().with_database(database).with_bundle_update().build();
    let result = {
        let mut executor = config.executor_for_block(&mut state, &sealed).unwrap();
        executor.apply_pre_execution_changes().unwrap();
        for tx in &block.body.transactions {
            executor.execute_transaction(Recovered::new_unchecked(tx, sender.address())).unwrap();
        }
        executor.apply_post_execution_changes().unwrap()
    };
    block.header.gas_used = result.gas_used;
    state.merge_transitions(BundleRetention::Reverts);
    let output = reth_provider::BlockExecutionOutput { state: state.take_bundle(), result };
    let outcome = ExecutionOutcome::single(0, output);
    let block = RecoveredBlock::new_unhashed(block, vec![sender.address(); 5]);
    let factory = create_test_provider_factory_with_chain_spec(chain);
    let writer = factory.database_provider_rw().unwrap();
    writer.insert_block(&block).unwrap();
    writer.commit().unwrap();
    let writer = factory.database_provider_rw().unwrap();
    writer
        .write_state(
            &outcome,
            OriginalValuesKnown::Yes,
            StateWriteConfig { write_receipts: false, ..Default::default() },
        )
        .unwrap();
    for (index, receipt) in outcome.receipts[0].iter().enumerate() {
        writer.tx_ref().put::<tables::Receipts>(index as u64, receipt.clone()).unwrap();
    }
    writer.commit().unwrap();
    (BlockchainProvider::new(factory).unwrap(), block, outcome, authority.address())
}

fn query(hash: B256, selections: Vec<ReceiptSelection>) -> QueryRequest {
    QueryRequest {
        block_hash: hash,
        object: "receipts".into(),
        selection: SelectionRequest { selections, include_proof: true },
    }
}

fn selection(path: &str, operation: SelectionOperation) -> ReceiptSelection {
    ReceiptSelection { path: path.into(), operation }
}

fn rpc_receipt(
    block: &RecoveredBlock<Block>,
    outcome: &ExecutionOutcome,
    index: usize,
) -> TransactionReceipt {
    let transaction = &block.body().transactions[index];
    let stored = outcome.receipts[0][index].clone();
    let previous = index.checked_sub(1).map_or(0, |i| outcome.receipts[0][i].cumulative_gas_used);
    let start =
        outcome.receipts[0][..index].iter().map(|receipt| receipt.logs.len()).sum::<usize>();
    let mut log_index = start;
    let inner = stored.map_logs(|log| {
        let position = log_index;
        log_index += 1;
        Log {
            inner: log,
            block_hash: Some(block.hash()),
            block_number: Some(block.header().number),
            block_timestamp: Some(block.header().timestamp),
            transaction_hash: Some(*transaction.hash()),
            transaction_index: Some(index as u64),
            log_index: Some(position as u64),
            removed: false,
        }
    });
    let inner: ReceiptEnvelope<Log> = inner.into();
    TransactionReceipt {
        inner,
        transaction_hash: *transaction.hash(),
        transaction_index: Some(index as u64),
        block_hash: Some(block.hash()),
        block_number: Some(block.header().number),
        gas_used: outcome.receipts[0][index].cumulative_gas_used - previous,
        effective_gas_price: 0,
        blob_gas_used: None,
        blob_gas_price: None,
        from: block.senders()[index],
        to: transaction.to(),
        contract_address: transaction
            .is_create()
            .then(|| block.senders()[index].create(transaction.nonce())),
    }
}

#[derive(Clone, Debug)]
struct LogRpcTypes;

impl reth_rpc_convert::RpcTypes for LogRpcTypes {
    type Header = alloy_rpc_types_eth::Header;
    type Receipt = TransactionReceipt;
    type Log = Log;
    type TransactionResponse = alloy_rpc_types_eth::Transaction;
    type TransactionRequest = alloy_rpc_types_eth::TransactionRequest;
}

#[derive(Clone, Debug)]
struct LocalLogConverter(
    reth_rpc_eth_types::receipt::EthReceiptConverter<reth_chainspec::ChainSpec>,
);

impl reth_rpc_convert::transaction::ReceiptConverter<reth_ethereum_primitives::EthPrimitives>
    for LocalLogConverter
{
    type RpcReceipt = TransactionReceipt;
    type RpcLog = Log;
    type Error = reth_rpc_eth_types::EthApiError;

    fn convert_log(
        &self,
        log: Log,
        _receipt: &reth_ethereum_primitives::Receipt,
        _header: &reth_primitives_traits::SealedHeader,
    ) -> Result<Log, Self::Error> {
        Ok(log)
    }

    fn convert_log_with_index(
        &self,
        mut log: Log,
        _receipt: &reth_ethereum_primitives::Receipt,
        _header: &reth_primitives_traits::SealedHeader,
        index: usize,
    ) -> Result<Log, Self::Error> {
        log.log_index = Some(index as u64);
        Ok(log)
    }

    fn convert_receipts(
        &self,
        inputs: Vec<
            reth_rpc_convert::transaction::ConvertReceiptInput<
                '_,
                reth_ethereum_primitives::EthPrimitives,
            >,
        >,
    ) -> Result<Vec<TransactionReceipt>, Self::Error> {
        self.0.convert_receipts(inputs)
    }
}

#[test]
fn both_log_filters_forward_original_positions_and_preserve_legacy_indices() {
    let (provider, block, outcome, _) = fixture();
    let chain = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
    let native = EthReceiptConverter::new(chain);
    let local =
        RpcConverter::<LogRpcTypes, EthEvmConfig, _>::new(LocalLogConverter(native.clone()));
    let legacy = RpcConverter::<LogRpcTypes, EthEvmConfig, _>::new(native);
    let address = Address::repeat_byte(0x7a);
    let mut receipts = outcome.receipts[0].clone();
    for receipt in &mut receipts {
        receipt.logs = (0..6)
            .map(|index| {
                alloy_primitives::Log::new_unchecked(
                    if index >= 4 { address } else { Address::ZERO },
                    vec![],
                    Bytes::from_static(b"duplicate"),
                )
            })
            .collect();
    }
    let filter = alloy_rpc_types_eth::Filter::new().address(address);
    let mut raw_header = block.header().clone();
    raw_header.logs_bloom = alloy_primitives::Bloom::repeat_byte(0xff);
    let header = reth_primitives_traits::SealedHeader::new(raw_header, block.hash());
    let hashes = block.body().transactions.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();
    for removed in [false, true] {
        let expected = matching_block_logs_with_tx_hashes(
            &local,
            &filter,
            &header,
            hashes.iter().copied().zip(&receipts),
            removed,
        )
        .unwrap();
        assert_eq!(
            expected.iter().map(|log| log.log_index.unwrap()).collect::<Vec<_>>(),
            vec![4, 5, 4, 5, 4, 5, 4, 5, 4, 5]
        );
        assert!(expected
            .iter()
            .all(|log| log.removed == removed && log.block_hash == Some(block.hash())));
        let mut from_block = vec![];
        append_matching_block_logs::<BlockchainProvider<MockNodeTypesWithDB>, _>(
            &mut from_block,
            &local,
            ProviderOrBlock::Block(Arc::new(block.clone())),
            &filter,
            &header,
            &receipts,
            removed,
        )
        .unwrap();
        assert_eq!(from_block, expected);
        let mut from_provider = vec![];
        append_matching_block_logs(
            &mut from_provider,
            &local,
            ProviderOrBlock::Provider(&provider),
            &filter,
            &header,
            &receipts,
            removed,
        )
        .unwrap();
        assert_eq!(from_provider, expected);
        let expected_legacy = matching_block_logs_with_tx_hashes(
            &legacy,
            &filter,
            &header,
            hashes.iter().copied().zip(&receipts),
            removed,
        )
        .unwrap();
        assert_eq!(
            expected_legacy.iter().map(|log| log.log_index.unwrap()).collect::<Vec<_>>(),
            vec![4, 5, 10, 11, 16, 17, 22, 23, 28, 29]
        );
        let mut legacy_logs = vec![];
        append_matching_block_logs(
            &mut legacy_logs,
            &legacy,
            ProviderOrBlock::Provider(&provider),
            &filter,
            &header,
            &receipts,
            removed,
        )
        .unwrap();
        assert_eq!(legacy_logs, expected_legacy);
        let mut no_match = vec![];
        append_matching_block_logs(
            &mut no_match,
            &local,
            ProviderOrBlock::Provider(&provider),
            &alloy_rpc_types_eth::Filter::new().address(Address::repeat_byte(0xee)),
            &header,
            &receipts,
            removed,
        )
        .unwrap();
        assert!(no_match.is_empty());
    }
}

#[test]
fn persisted_execution_receipts_join_queries_filters_and_shared_proofs() {
    let (provider, block, _, authority) = fixture();
    let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
    println!(
        "EXECUTION_RECEIPTS {}",
        serde_json::json!({"bytes": snapshot.receipt_snapshot().serialized(), "root": snapshot.root()})
    );
    assert_eq!(
        snapshot.receipt_snapshot().receipts().get(0).unwrap().authorities(),
        Some([authority, authority, authority, Address::ZERO].as_slice())
    );
    assert_eq!(
        snapshot.receipt_snapshot().receipts().get(1).unwrap().authorities(),
        Some([authority, authority].as_slice())
    );
    assert!(!snapshot.receipt_snapshot().receipts().get(1).unwrap().status());
    assert_eq!(
        snapshot.receipt_snapshot().receipts().get(2).unwrap().contract_address(),
        Some(Address::ZERO)
    );
    let request = query(
        block.hash(),
        vec![
            selection("[0].authorities", SelectionOperation::Range { start: 0, end: 4 }),
            selection("[1].authorities[1]", SelectionOperation::Value {}),
            selection("[1].status", SelectionOperation::Value {}),
            selection("[0].logs[0].data", SelectionOperation::Slice { start: 0, end: 32 }),
            selection("[2].contract_address", SelectionOperation::Value {}),
        ],
    );
    let service = QueryService::from_blockchain_provider(provider);
    let response = service.query(request.clone()).unwrap();
    verify_query_response(&request, &response, snapshot.root(), SelectionLimits::default())
        .unwrap();
    assert_eq!(
        response.selection.results[0].values_ssz,
        vec![
            Bytes::copy_from_slice(authority.as_slice()),
            Bytes::copy_from_slice(authority.as_slice()),
            Bytes::copy_from_slice(authority.as_slice()),
            Bytes::copy_from_slice(Address::ZERO.as_slice())
        ]
    );
    assert!(response.selection.multiproof.is_some());
    let filter = query(
        block.hash(),
        vec![selection(
            ".",
            SelectionOperation::Filter {
                output: FilterOutput::Receipts,
                predicates: vec![FilterPredicate::Status { value: false }],
            },
        )],
    );
    let response = service.query(filter.clone()).unwrap();
    assert_eq!(response.selection.results[0].values_ssz.len(), 2);
    verify_query_response(&filter, &response, snapshot.root(), SelectionLimits::default()).unwrap();
    let mut omitted = response;
    omitted.selection.results[0].values_ssz.pop();
    assert!(verify_query_response(&filter, &omitted, snapshot.root(), SelectionLimits::default())
        .is_err());
}

#[test]
fn stored_receipt_failures_reject_queries_without_mutating_acquired_snapshots() {
    let (provider, block, outcome, _) = fixture();
    let request = query(block.hash(), vec![selection("[3].status", SelectionOperation::Value {})]);
    let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
    let service = QueryService::from_blockchain_provider(provider.clone());
    let mut invalid = outcome.receipts[0][0].clone();
    invalid.tx_type = alloy_consensus::TxType::Legacy;
    let writer = provider.database_provider_rw().unwrap();
    writer.tx_ref().put::<tables::Receipts>(0, invalid).unwrap();
    writer.commit().unwrap();
    assert!(matches!(
        service.query(request.clone()),
        Err(QueryError::Acquisition(HistoricalAcquisitionError::Snapshot(
            reth_pureth_receipt::Eip6466SnapshotError::Conversion(_)
        )))
    ));
    let writer = provider.database_provider_rw().unwrap();
    for index in 0..5 {
        writer.tx_ref().delete::<tables::Receipts>(index, None).unwrap();
    }
    writer.commit().unwrap();
    assert!(matches!(
        service.query(request.clone()),
        Err(QueryError::Acquisition(ref cause)) if cause.is_unavailable()
    ));
    let retained = query_snapshot(&request, &snapshot, SelectionLimits::default()).unwrap();
    verify_query_response(&request, &retained, snapshot.root(), SelectionLimits::default())
        .unwrap();
    assert!(matches!(
        service.query_with_cancel(request, &AtomicBool::new(true)),
        Err(QueryError::Selection(SelectionError::Cancelled))
    ));
}

#[test]
fn native_receipt_fields_are_bound_and_legacy_responses_stay_unchanged() {
    let (provider, block, outcome, authority) = fixture();
    let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
    let limits = SelectionLimits::default();
    let mut values = Vec::new();
    let legacy_converter = reth_pureth_query::ExecutionReceiptConverter::new(
        Arc::new(ChainSpecBuilder::mainnet().prague_activated().build()),
        reth_pureth_block::ExecutionForkConfig::new(u64::MAX),
        limits,
    );
    let proof_converter = legacy_converter.clone().with_receipt_proof(true);
    for index in 0..5 {
        let native = rpc_receipt(&block, &outcome, index);
        let legacy_input = || ConvertReceiptInput {
            receipt: outcome.receipts[0][index].clone(),
            tx: Recovered::new_unchecked(&block.body().transactions[index], block.senders()[index]),
            gas_used: native.gas_used,
            next_log_index: outcome.receipts[0][..index]
                .iter()
                .map(|receipt| receipt.logs.len())
                .sum(),
            meta: reth_primitives_traits::TransactionMeta {
                tx_hash: native.transaction_hash,
                index: index as u64,
                block_hash: block.hash(),
                block_number: block.header().number,
                timestamp: block.header().timestamp,
                base_fee: block.header().base_fee_per_gas,
                excess_blob_gas: block.header().excess_blob_gas,
            },
        };
        let legacy_receipts = legacy_converter.convert_receipts(vec![legacy_input()]).unwrap();
        assert_eq!(
            serde_json::to_value(&legacy_receipts[0]).unwrap(),
            serde_json::to_value(&native).unwrap()
        );
        let null_receipts = proof_converter.convert_receipts(vec![legacy_input()]).unwrap();
        let null_json = serde_json::to_value(&null_receipts[0]).unwrap();
        assert!(null_json.as_object().unwrap().contains_key("sszProof"));
        assert!(null_json["sszProof"].is_null());
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<reth_pureth_query::ExecutionRpcReceipt>(null_json.clone())
                    .unwrap()
            )
            .unwrap(),
            null_json
        );
        let mut expected_json = serde_json::to_value(&native).unwrap();
        let expected = snapshot.receipt_snapshot().receipts().get(index).unwrap();
        let object = expected_json.as_object_mut().unwrap();
        object.remove("logsBloom");
        object.insert(
            "contractAddress".into(),
            serde_json::to_value(expected.contract_address()).unwrap(),
        );
        object.insert("authorities".into(), serde_json::to_value(expected.authorities()).unwrap());
        for (position, log) in object["logs"].as_array_mut().unwrap().iter_mut().enumerate() {
            log["logIndex"] = serde_json::json!(format!("0x{position:x}"));
        }
        let legacy = extend_receipt_response(
            native.clone(),
            index as u64,
            ReceiptProofSupport::Supported(&snapshot),
            false,
            limits,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), serde_json::to_value(&native).unwrap());
        let value = convert_eip6466_receipt(
            native.clone(),
            &block,
            &snapshot,
            index as u64,
            limits,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            convert_eip6466_receipt_from_retained(
                native.clone(),
                &block,
                snapshot.receipt_snapshot(),
                index as u64,
                limits,
                &AtomicBool::new(false),
            )
            .unwrap(),
            value
        );
        assert_eq!(value, expected_json);
        let typed: reth_pureth_query::ExecutionRpcReceipt =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), value);
        let legacy_json = serde_json::to_value(&native).unwrap();
        let legacy_typed: reth_pureth_query::ExecutionRpcReceipt =
            serde_json::from_value(legacy_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy_typed).unwrap(), legacy_json);
        assert!(value.get("logsBloom").is_none());
        assert_eq!(value["cumulativeGasUsed"], expected_json["cumulativeGasUsed"]);
        if index == 0 {
            assert_eq!(
                value["authorities"],
                serde_json::json!([authority, authority, authority, Address::ZERO])
            );
        }
        if index == 2 {
            assert_eq!(value["contractAddress"], serde_json::json!(Address::ZERO));
        }
        if index == 3 {
            assert_eq!(value["logs"][0]["logIndex"], "0x0");
        }
        values.push(value);
    }
    for (index, field, value) in [
        (0, "status", serde_json::json!("0x2")),
        (0, "logsBloom", serde_json::json!(alloy_primitives::Bloom::ZERO)),
        (0, "type", serde_json::json!("0x5")),
        (0, "authorities", serde_json::Value::Null),
        (3, "authorities", serde_json::json!([])),
    ] {
        let mut invalid = values[index].clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<reth_pureth_query::ExecutionRpcReceipt>(invalid).is_err(),
            "index={index}, field={field}"
        );
    }
    let native = (0..5).map(|index| rpc_receipt(&block, &outcome, index)).collect::<Vec<_>>();
    assert_eq!(
        convert_eip6466_block_receipts(
            native.clone(),
            &block,
            &snapshot,
            limits,
            &AtomicBool::new(false)
        )
        .unwrap(),
        values
    );
    let mut reordered = native.clone();
    reordered.swap(0, 1);
    assert!(convert_eip6466_block_receipts(
        reordered,
        &block,
        &snapshot,
        limits,
        &AtomicBool::new(false)
    )
    .is_err());
    assert!(convert_eip6466_block_receipts(
        native[..3].to_vec(),
        &block,
        &snapshot,
        limits,
        &AtomicBool::new(false)
    )
    .is_err());
    let response_bytes =
        native.iter().map(|receipt| serde_json::to_vec(receipt).unwrap().len()).max().unwrap();
    assert!(convert_eip6466_block_receipts(
        native,
        &block,
        &snapshot,
        SelectionLimits { response_bytes, ..limits },
        &AtomicBool::new(false)
    )
    .is_err());
    let responses = extend_block_receipt_responses(
        values,
        ReceiptProofSupport::Supported(&snapshot),
        true,
        limits,
        &AtomicBool::new(false),
    )
    .unwrap();
    for (index, response) in responses.iter().enumerate() {
        let ReceiptResponse::WithProof { receipt, ssz_proof: Some(proof) } = response else {
            panic!("expected proof")
        };
        let verify = |receipt: &serde_json::Value, limits| {
            verify_eip6466_receipt_response(
                receipt,
                proof,
                block.hash(),
                index as u64,
                *block.body().transactions[index].hash(),
                snapshot.root(),
                limits,
            )
        };
        verify(receipt, limits).unwrap();
        let response_bytes = serde_json::to_vec(response).unwrap().len();
        verify(receipt, SelectionLimits { response_bytes, ..limits }).unwrap();
        assert!(verify(receipt, SelectionLimits { response_bytes: response_bytes - 1, ..limits })
            .is_err());
        for (field, value) in [
            ("from", serde_json::json!(Address::ZERO)),
            ("gasUsed", serde_json::json!("0x0")),
            ("status", serde_json::json!("0x2")),
            ("contractAddress", serde_json::json!(Address::repeat_byte(0x99))),
            ("authorities", serde_json::json!([Address::repeat_byte(0x99)])),
            ("blockHash", serde_json::json!(B256::ZERO)),
            ("transactionHash", serde_json::json!(B256::ZERO)),
            ("transactionIndex", serde_json::json!("0xff")),
            ("logsBloom", serde_json::json!("0x00")),
            ("root", serde_json::json!(B256::ZERO)),
        ] {
            let mut changed = receipt.clone();
            changed[field] = value;
            assert!(verify(&changed, limits).is_err(), "{field}");
        }
        if index == 0 {
            let mut changed = receipt.clone();
            changed["cumulativeGasUsed"] = serde_json::json!("not a quantity");
            assert!(verify(&changed, limits).is_err());
            changed.as_object_mut().unwrap().remove("cumulativeGasUsed");
            assert!(verify(&changed, limits).is_err());
            changed["cumulativeGasUsed"] = serde_json::json!("0xffff");
            verify(&changed, limits).unwrap();
        }
        if index == 0 || index == 3 {
            for field in ["address", "data", "topics", "logIndex", "removed"] {
                let mut changed = receipt.clone();
                changed["logs"][0][field] = match field {
                    "address" => serde_json::json!(Address::ZERO),
                    "data" | "logIndex" => serde_json::json!("0xff"),
                    "topics" => serde_json::json!([]),
                    _ => serde_json::json!(true),
                };
                assert!(verify(&changed, limits).is_err());
            }
        }
    }
    for mutation in 0..5 {
        let mut native = rpc_receipt(&block, &outcome, 0);
        match mutation {
            0 => native.block_hash = Some(B256::ZERO),
            1 => native.transaction_hash = B256::ZERO,
            2 => native.transaction_index = Some(1),
            3 => native.gas_used += 1,
            _ => native.from = Address::ZERO,
        }
        assert!(convert_eip6466_receipt(
            native,
            &block,
            &snapshot,
            0,
            limits,
            &AtomicBool::new(false)
        )
        .is_err());
    }
    for limits in [
        SelectionLimits { source_bytes: 0, ..Default::default() },
        SelectionLimits { response_bytes: 1, ..Default::default() },
    ] {
        assert!(convert_eip6466_receipt(
            rpc_receipt(&block, &outcome, 0),
            &block,
            &snapshot,
            0,
            limits,
            &AtomicBool::new(false)
        )
        .is_err());
    }
    assert_eq!(
        convert_eip6466_receipt(
            rpc_receipt(&block, &outcome, 0),
            &block,
            &snapshot,
            0,
            limits,
            &AtomicBool::new(true)
        )
        .unwrap_err(),
        SelectionError::Cancelled
    );
}

#[tokio::test]
async fn installed_query_rpc_preserves_execution_backed_success_and_source_errors() {
    let (provider, block, outcome, _) = fixture();
    let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
    let request =
        query(block.hash(), vec![selection("[0].authorities[1]", SelectionOperation::Value {})]);
    let module: Methods =
        PurethRpc::new(QueryService::from_blockchain_provider(provider.clone())).into_rpc().into();
    let raw = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "pureth_query", "params": {"request": request}}).to_string();
    let (response, _) = module.raw_json_request(&raw, 1).await.unwrap();
    let reply: serde_json::Value = serde_json::from_str(response.get()).unwrap();
    let response: QueryResponse = serde_json::from_value(reply["result"].clone()).unwrap();
    verify_query_response(&request, &response, snapshot.root(), SelectionLimits::default())
        .unwrap();
    let writer = provider.database_provider_rw().unwrap();
    let mut invalid = outcome.receipts[0][0].clone();
    invalid.tx_type = alloy_consensus::TxType::Legacy;
    writer.tx_ref().put::<tables::Receipts>(0, invalid).unwrap();
    writer.commit().unwrap();
    let (response, _) = module.raw_json_request(&raw, 1).await.unwrap();
    let reply: serde_json::Value = serde_json::from_str(response.get()).unwrap();
    assert_eq!(reply["error"]["code"], -32603);
    assert!(reply.get("result").is_none());
    let writer = provider.database_provider_rw().unwrap();
    for index in 0..5 {
        writer.tx_ref().delete::<tables::Receipts>(index, None).unwrap();
    }
    writer.commit().unwrap();
    let (response, _) = module.raw_json_request(&raw, 1).await.unwrap();
    let reply: serde_json::Value = serde_json::from_str(response.get()).unwrap();
    assert_eq!(reply["error"]["code"], -32001);
    assert!(reply.get("result").is_none());
}

#[test]
fn native_conversion_rejects_aggregate_expansion_before_later_receipts() {
    let (provider, block, outcome, _) = fixture_with_additional_authorizations(100);
    let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
    let native = (0..5).map(|index| rpc_receipt(&block, &outcome, index)).collect::<Vec<_>>();
    let limits = SelectionLimits::default();
    let cancelled = AtomicBool::new(false);
    let values =
        convert_eip6466_block_receipts(native.clone(), &block, &snapshot, limits, &cancelled)
            .unwrap();
    let native_bytes = serde_json::to_vec(&native).unwrap().len();
    let first_bytes = serde_json::to_vec(&values[0]).unwrap().len();
    let largest_item =
        values.iter().map(|value| serde_json::to_vec(value).unwrap().len()).max().unwrap();
    let response_bytes = native_bytes.max(largest_item) + 1;
    assert!(serde_json::to_vec(&values[..2]).unwrap().len() > response_bytes);
    let mut invalid_last = native.clone();
    invalid_last[4].gas_used += 1;
    assert_eq!(
        convert_eip6466_block_receipts(
            invalid_last.clone(),
            &block,
            &snapshot,
            SelectionLimits { response_bytes, ..limits },
            &cancelled,
        )
        .unwrap_err(),
        SelectionError::LimitExceeded("response bytes")
    );
    let first_native = serde_json::to_vec(&native[0]).unwrap().len();
    assert!(first_bytes > first_native);
    let mut lower = 0;
    let mut upper = limits.source_bytes;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        match convert_eip6466_receipt(
            native[0].clone(),
            &block,
            &snapshot,
            0,
            SelectionLimits { source_bytes: middle, ..limits },
            &cancelled,
        ) {
            Ok(_) => upper = middle,
            Err(SelectionError::LimitExceeded("source bytes")) => lower = middle + 1,
            result => panic!("unexpected budget result: {result:?}"),
        }
    }
    let source = lower - 2 * first_bytes;
    let source_bytes = source + 2 * native_bytes.max(first_bytes + 2);
    assert_eq!(
        convert_eip6466_block_receipts(
            invalid_last,
            &block,
            &snapshot,
            SelectionLimits { source_bytes, ..limits },
            &cancelled,
        )
        .unwrap_err(),
        SelectionError::LimitExceeded("source bytes")
    );
    let complete_bytes = serde_json::to_vec(&values).unwrap().len();
    for (response_bytes, success) in [(complete_bytes, true), (complete_bytes - 1, false)] {
        assert_eq!(
            convert_eip6466_block_receipts(
                native.clone(),
                &block,
                &snapshot,
                SelectionLimits { response_bytes, ..limits },
                &cancelled,
            )
            .is_ok(),
            success
        );
    }
    for (response_bytes, success) in [(first_bytes, true), (first_bytes - 1, false)] {
        assert_eq!(
            convert_eip6466_receipt(
                native[0].clone(),
                &block,
                &snapshot,
                0,
                SelectionLimits { response_bytes, ..limits },
                &cancelled,
            )
            .is_ok(),
            success
        );
    }
    let source_bytes = source + 2 * complete_bytes.max(native_bytes);
    assert_eq!(
        convert_eip6466_block_receipts(
            native.clone(),
            &block,
            &snapshot,
            SelectionLimits { source_bytes, ..limits },
            &cancelled,
        )
        .unwrap(),
        values
    );
    assert_eq!(
        convert_eip6466_block_receipts(
            native,
            &block,
            &snapshot,
            SelectionLimits { source_bytes: source_bytes - 1, ..limits },
            &cancelled,
        )
        .unwrap_err(),
        SelectionError::LimitExceeded("source bytes")
    );
}
