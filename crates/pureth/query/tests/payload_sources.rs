#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![cfg(not(target_arch = "wasm32"))]

use alloy_consensus::{BlockHeader, SignableTransaction, TxEip7702, TxLegacy, TxReceipt};
use alloy_eips::{
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
    eip4895::Withdrawal,
    eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE},
    eip7702::Authorization,
};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use jsonrpsee::core::server::Methods;
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_chainspec::ChainSpecBuilder;
use reth_db_common::init::init_genesis;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{execute::BlockBuilder, ConfigureEvm, NextBlockEnvAttributes};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{BlockBody, Recovered, SealedHeader, TransactionMeta};
use reth_provider::{
    providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    BalConfig, BalStore, BlockExecutionOutput, EvmStateProviderAdapter, InMemoryBalStore, RawBal,
};
use reth_pureth_block::{
    ExecutionBlock, ExecutionBody, ExecutionForkConfig, ExecutionHeader, ExecutionSource,
    ExecutionSourceProvider, PayloadError, PayloadLimits, PayloadSourceError, PayloadSourceInput,
    SourcePayloadSnapshot,
};
use reth_pureth_query::{
    convert_eip6466_block_receipts_from_retained, convert_eip6466_receipt_from_retained,
    extend_block_receipt_responses, extend_receipt_response, link_component, select_component,
    select_receipt_multiproof, verify_component_link, verify_component_selection,
    verify_eip6466_receipt_response, verify_query_response, verify_receipt_selection,
    verify_transaction_receipt, ComponentObject, ComponentSnapshot, ExecutionHeaderConverter,
    ExecutionReceiptConverter, ExecutionRpcReceipt, FilterOutput, FilterPredicate, PurethApiServer,
    PurethRpc, QueryError, QueryHandler, QueryRequest, QueryResponse, QueryService,
    ReceiptProofSupport, ReceiptResponse, ReceiptSelection, ResponseVerificationError,
    SelectionError, SelectionLimits, SelectionOperation, SelectionRequest,
    TransactionReceiptRequest, TransactionReceiptResponse,
};
use reth_pureth_receipt::LookupError;
use reth_revm::database::StateProviderDatabase;
use reth_rpc_convert::transaction::{ConvertReceiptInput, HeaderConverter, ReceiptConverter};
use revm::database::State;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Debug)]
struct RetainedSources {
    receipts: Arc<ExecutionSource>,
    payload: Arc<ExecutionSource>,
    receipt_calls: AtomicUsize,
    payload_calls: AtomicUsize,
}

impl ExecutionSourceProvider for RetainedSources {
    fn receipt_source(
        &self,
        block: alloy_eips::BlockId,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        self.receipt_calls.fetch_add(1, Ordering::Relaxed);
        self.acquire(&self.receipts, block, cancelled)
    }

    fn payload_source(
        &self,
        block: alloy_eips::BlockId,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        self.payload_calls.fetch_add(1, Ordering::Relaxed);
        self.acquire(&self.payload, block, cancelled)
    }
}

impl RetainedSources {
    fn acquire(
        &self,
        source: &Arc<ExecutionSource>,
        block: alloy_eips::BlockId,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(PayloadSourceError::Cancelled);
        }
        if !matches!(block, alloy_eips::BlockId::Hash(hash) if hash.block_hash == source.block().hash())
        {
            return Err(PayloadSourceError::MissingInput(PayloadSourceInput::Block));
        }
        if matches!(block, alloy_eips::BlockId::Hash(hash) if hash.require_canonical == Some(true)) &&
            source.metadata().canonicality != reth_pureth_receipt::CanonicalityStatus::Canonical
        {
            return Err(PayloadSourceError::NonCanonicalBlock(source.block().num_hash()));
        }
        Ok(source.clone())
    }
}

#[derive(Clone, Debug)]
struct CancellingBalStore {
    cancelled: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
}

impl BalStore for CancellingBalStore {
    fn insert(
        &self,
        _: alloy_eips::NumHash,
        _: RawBal,
    ) -> Result<(), reth_provider::ProviderError> {
        Err(reth_provider::ProviderError::UnsupportedProvider)
    }

    fn prune(&self, _: u64) -> Result<usize, reth_provider::ProviderError> {
        Err(reth_provider::ProviderError::UnsupportedProvider)
    }

    fn get_by_hashes(
        &self,
        hashes: &[B256],
    ) -> Result<Vec<Option<Bytes>>, reth_provider::ProviderError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.cancelled.store(true, Ordering::Relaxed);
        Ok(vec![Some(Bytes::from_static(&[0x80])); hashes.len()])
    }
}

#[tokio::test]
async fn real_payload_sources_feed_component_queries_links_and_rpc() {
    let sender: PrivateKeySigner = B256::repeat_byte(9).to_string().parse().unwrap();
    let authority: PrivateKeySigner = B256::repeat_byte(1).to_string().parse().unwrap();
    let recipient = Address::repeat_byte(0x44);
    let withdrawal_recipient = Address::repeat_byte(0x55);
    let mut genesis = Genesis {
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1_000_000_000),
        ..Default::default()
    };
    for address in [sender.address(), authority.address(), recipient, withdrawal_recipient] {
        genesis.alloc.insert(
            address,
            GenesisAccount { balance: U256::from(10u64.pow(18)), ..Default::default() },
        );
    }
    for (address, code) in [
        (HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE.clone()),
        (BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE.clone()),
        (WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone()),
        (CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE.clone()),
    ] {
        genesis.alloc.insert(
            address,
            GenesisAccount { nonce: Some(1), code: Some(code), ..Default::default() },
        );
    }
    let spec = Arc::new(ChainSpecBuilder::mainnet().genesis(genesis).amsterdam_activated().build());
    let factory = create_test_provider_factory_with_chain_spec(spec.clone());
    init_genesis(&factory).unwrap();
    let parent = SealedHeader::seal_slow(spec.genesis_header().clone());
    let attributes = NextBlockEnvAttributes {
        timestamp: 12,
        suggested_fee_recipient: Address::repeat_byte(0x66),
        prev_randao: B256::repeat_byte(0x77),
        gas_limit: parent.gas_limit,
        parent_beacon_block_root: Some(B256::repeat_byte(0x88)),
        withdrawals: Some(
            vec![Withdrawal {
                index: 1,
                validator_index: 2,
                address: withdrawal_recipient,
                amount: 3,
            }]
            .into(),
        ),
        extra_data: Bytes::from_static(b"execution-source"),
        slot_number: Some(1),
    };
    let state_provider = factory.latest().unwrap();
    let evm_source = EvmStateProviderAdapter(state_provider.as_ref());
    let mut state = State::builder()
        .with_database(StateProviderDatabase::new(&evm_source))
        .with_bundle_update()
        .with_bal_builder()
        .build();
    let config = EthEvmConfig::ethereum(spec.clone());
    let mut builder =
        config.builder_for_next_block(&mut state, &parent, attributes.clone()).unwrap();
    builder.apply_pre_execution_changes().unwrap();
    let transaction = TxLegacy {
        chain_id: Some(1),
        gas_limit: 100_000,
        gas_price: 2_000_000_000,
        to: TxKind::Call(recipient),
        value: U256::from(1),
        ..Default::default()
    };
    let signature = sender.sign_hash_sync(&transaction.signature_hash()).unwrap();
    let transaction: TransactionSigned = transaction.into_signed(signature).into();
    builder.execute_transaction(Recovered::new_unchecked(transaction, sender.address())).unwrap();
    let (outcome, fresh) = SourcePayloadSnapshot::finish_builder(
        builder,
        state_provider.as_ref(),
        &parent,
        &attributes,
        spec.as_ref(),
        PayloadLimits::default(),
    )
    .unwrap();
    let expected_root =
        alloy_primitives::b256!("52453dd36381348f20d4bfa8a994409cd10310ce96addb7107043c66e26c9e72");
    assert_eq!(fresh.root(), expected_root);
    let cancelled = AtomicBool::new(false);
    let source = SourcePayloadSnapshot::from_builder_outcome(
        &outcome,
        &parent,
        &attributes,
        spec.as_ref(),
        reth_pureth_block::StateRootStatus::Computed,
        PayloadLimits::default(),
    )
    .unwrap()
    .into_execution_source(
        outcome.block.clone(),
        outcome.execution_result.receipts.clone(),
        spec.as_ref(),
        &ExecutionForkConfig::new(12),
        &cancelled,
    )
    .unwrap();
    let source = Arc::new(source);
    assert_eq!(source.block().hash(), expected_root);
    assert_runtime_receipt_conversion(source.clone(), spec.clone());
    let narrow = Arc::new(
        ExecutionSource::from_receipts(
            source.block_arc().clone(),
            source.native_receipts_arc().clone(),
            *source.metadata(),
            spec.as_ref(),
            &ExecutionForkConfig::new(12),
            &cancelled,
        )
        .unwrap(),
    );
    assert!(matches!(
        narrow.payload(),
        Err(PayloadSourceError::MissingInput(PayloadSourceInput::FullPayload))
    ));
    let backend = Arc::new(RetainedSources {
        receipts: narrow.clone(),
        payload: source.clone(),
        receipt_calls: AtomicUsize::new(0),
        payload_calls: AtomicUsize::new(0),
    });
    let service = QueryService::from_execution_source(backend.clone());
    let (mut retained_cases, summary_bytes) = query_cases(&fresh);
    for (request, _) in &mut retained_cases {
        request.block_hash = expected_root;
    }
    let receipt_case = &retained_cases[5];
    let response = service.query(receipt_case.0.clone()).unwrap();
    verify_query_response(&receipt_case.0, &response, receipt_case.1, SelectionLimits::default())
        .unwrap();
    assert_eq!(backend.receipt_calls.load(Ordering::Relaxed), 1);
    assert_eq!(backend.payload_calls.load(Ordering::Relaxed), 0);
    let counts = (
        backend.receipt_calls.load(Ordering::Relaxed),
        backend.payload_calls.load(Ordering::Relaxed),
    );
    assert!(matches!(
        service.query_with_cancel(retained_cases[0].0.clone(), &AtomicBool::new(true)),
        Err(QueryError::Selection(SelectionError::Cancelled))
    ));
    assert_eq!(
        counts,
        (
            backend.receipt_calls.load(Ordering::Relaxed),
            backend.payload_calls.load(Ordering::Relaxed)
        )
    );
    assert_query_service(service, &retained_cases, &summary_bytes, "unknown").await;
    let received = Arc::new(
        ExecutionSource::from_execution(
            source.block_arc().clone(),
            &outcome.execution_result,
            outcome.block_access_list.as_ref().unwrap().as_bal(),
            spec.as_ref(),
            &ExecutionForkConfig::new(12),
            PayloadLimits::default(),
        )
        .unwrap(),
    );
    assert_eq!(received.metadata().source_kind, reth_pureth_block::PayloadSourceKind::Received);
    assert!(Arc::ptr_eq(received.block_arc(), source.block_arc()));
    assert_eq!(received.payload().unwrap().serialized(), source.payload().unwrap().serialized());
    assert_runtime_receipt_conversion(received.clone(), spec.clone());
    let received_backend = Arc::new(RetainedSources {
        receipts: received.clone(),
        payload: received,
        receipt_calls: AtomicUsize::new(0),
        payload_calls: AtomicUsize::new(0),
    });
    assert_query_service(
        QueryService::from_execution_source(received_backend),
        &retained_cases,
        &summary_bytes,
        "unknown",
    )
    .await;
    let narrow_backend = Arc::new(RetainedSources {
        receipts: narrow.clone(),
        payload: narrow,
        receipt_calls: AtomicUsize::new(0),
        payload_calls: AtomicUsize::new(0),
    });
    let module: Methods =
        PurethRpc::new(QueryService::from_execution_source(narrow_backend)).into_rpc().into();
    assert!(rpc_query(&module, &receipt_case.0).await.get("result").is_some());
    assert_eq!(rpc_query(&module, &retained_cases[0].0).await["error"]["code"], -32001);
    assert_eq!(
        rpc_association(
            &module,
            &TransactionReceiptRequest { block_hash: expected_root, transaction_index: 0 }
        )
        .await["error"]["code"],
        -32001
    );
    let header = ExecutionHeader::from_payload(fresh.payload()).unwrap();
    let body = ExecutionBody::from_payload(
        fresh.payload(),
        alloy_consensus::BlockBody {
            transactions: outcome.block.body().transactions.clone(),
            withdrawals: outcome.block.body().withdrawals.clone(),
            ommers: Vec::new(),
        },
    )
    .unwrap();
    let adopted =
        reth_primitives_traits::Block::seal_slow(ExecutionBlock::try_new(header, body).unwrap())
            .try_recover()
            .unwrap();
    let retained = &fresh.payload().components().receipts;
    let receipt = alloy_rpc_types_eth::TransactionReceipt {
        inner: outcome.execution_result.receipts[0]
            .clone()
            .map_logs(|log| alloy_rpc_types_eth::Log {
                inner: log,
                block_hash: Some(adopted.hash()),
                block_number: Some(1),
                block_timestamp: Some(attributes.timestamp),
                transaction_hash: Some(*outcome.block.body().transactions[0].tx_hash()),
                transaction_index: Some(0),
                log_index: Some(0),
                removed: false,
            })
            .into(),
        transaction_hash: *outcome.block.body().transactions[0].tx_hash(),
        transaction_index: Some(0),
        block_hash: Some(adopted.hash()),
        block_number: Some(1),
        gas_used: retained.receipts().get(0).unwrap().gas_used(),
        effective_gas_price: 2_000_000_000,
        blob_gas_used: None,
        blob_gas_price: None,
        from: sender.address(),
        to: Some(recipient),
        contract_address: None,
    };
    let cancelled = AtomicBool::new(false);
    let converted = convert_eip6466_receipt_from_retained(
        receipt.clone(),
        &adopted,
        retained,
        0,
        SelectionLimits::default(),
        &cancelled,
    )
    .unwrap();
    assert_eq!(converted["blockHash"], serde_json::to_value(expected_root).unwrap());
    assert!(converted.get("logsBloom").is_none());
    assert_eq!(
        converted["cumulativeGasUsed"],
        serde_json::to_value(&receipt).unwrap()["cumulativeGasUsed"]
    );
    assert_eq!(
        convert_eip6466_block_receipts_from_retained(
            vec![receipt.clone()],
            &adopted,
            retained,
            SelectionLimits::default(),
            &cancelled,
        )
        .unwrap(),
        vec![converted.clone()]
    );
    let support = ReceiptProofSupport::Retained { block_hash: adopted.hash(), snapshot: retained };
    let proved =
        extend_receipt_response(converted.clone(), 0, support, true, SelectionLimits::default())
            .unwrap();
    let ReceiptResponse::WithProof { ssz_proof: Some(proof), .. } = &proved else {
        panic!("retained receipt source must provide the requested proof")
    };
    verify_eip6466_receipt_response(
        &converted,
        proof,
        adopted.hash(),
        0,
        receipt.transaction_hash,
        retained.root(),
        SelectionLimits::default(),
    )
    .unwrap();
    let batch = extend_block_receipt_responses(
        vec![converted],
        support,
        true,
        SelectionLimits::default(),
        &cancelled,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(batch).unwrap(), serde_json::json!([proved]));
    let mut wrong_identity = receipt.clone();
    wrong_identity.block_hash = Some(outcome.block.hash());
    assert_eq!(
        convert_eip6466_receipt_from_retained(
            wrong_identity,
            &adopted,
            retained,
            0,
            SelectionLimits::default(),
            &cancelled,
        ),
        Err(SelectionError::InvalidContext)
    );
    assert_eq!(
        convert_eip6466_receipt_from_retained(
            receipt,
            &adopted,
            retained,
            0,
            SelectionLimits::default(),
            &AtomicBool::new(true),
        ),
        Err(SelectionError::Cancelled)
    );
    assert_eq!(fresh.root(), expected_root);
    let mut alternate_attributes = attributes.clone();
    alternate_attributes.timestamp = 24;
    alternate_attributes.slot_number = Some(2);
    let mut alternate_state = State::builder()
        .with_database(StateProviderDatabase::new(&evm_source))
        .with_bundle_update()
        .with_bal_builder()
        .build();
    let mut alternate_builder = config
        .builder_for_next_block(&mut alternate_state, &parent, alternate_attributes.clone())
        .unwrap();
    alternate_builder.apply_pre_execution_changes().unwrap();
    alternate_builder
        .execute_transaction(Recovered::new_unchecked(
            outcome.block.body().transactions[0].clone(),
            sender.address(),
        ))
        .unwrap();
    let transaction = TxLegacy {
        chain_id: Some(1),
        nonce: 1,
        gas_limit: 100_000,
        gas_price: 2_000_000_000,
        value: U256::from(2),
        to: TxKind::Call(withdrawal_recipient),
        ..Default::default()
    };
    let signature = sender.sign_hash_sync(&transaction.signature_hash()).unwrap();
    alternate_builder
        .execute_transaction(Recovered::new_unchecked(
            TransactionSigned::from(transaction.into_signed(signature)),
            sender.address(),
        ))
        .unwrap();
    for (nonce, input) in [
        (2, Bytes::from_static(&[0x60, 0, 0x60, 0, 0xfd])),
        (3, Bytes::from_static(&[0x60, 1, 0x60, 0, 0xf3])),
    ] {
        let transaction = TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_limit: 1_000_000,
            gas_price: 2_000_000_000,
            to: TxKind::Create,
            input,
            ..Default::default()
        };
        let signature = sender.sign_hash_sync(&transaction.signature_hash()).unwrap();
        alternate_builder
            .execute_transaction(Recovered::new_unchecked(
                TransactionSigned::from(transaction.into_signed(signature)),
                sender.address(),
            ))
            .unwrap();
    }
    let authorization = |chain_id| {
        let authorization =
            Authorization { chain_id: U256::from(chain_id), nonce: 0, address: recipient };
        let signature = authority.sign_hash_sync(&authorization.signature_hash()).unwrap();
        authorization.into_signed(signature)
    };
    let transaction = TxEip7702 {
        chain_id: 1,
        nonce: 4,
        gas_limit: 500_000,
        max_fee_per_gas: 2_000_000_000,
        to: recipient,
        authorization_list: vec![
            authorization(2u64),
            authorization(0),
            authorization(0),
            Authorization { chain_id: U256::from(1), nonce: 0, address: recipient }
                .into_signed(alloy_primitives::Signature::new(U256::ZERO, U256::from(1), false)),
        ],
        ..Default::default()
    };
    let signature = sender.sign_hash_sync(&transaction.signature_hash()).unwrap();
    alternate_builder
        .execute_transaction(Recovered::new_unchecked(
            TransactionSigned::from(transaction.into_signed(signature)),
            sender.address(),
        ))
        .unwrap();
    let (alternate_outcome, alternate_source) = SourcePayloadSnapshot::finish_builder(
        alternate_builder,
        state_provider.as_ref(),
        &parent,
        &alternate_attributes,
        spec.as_ref(),
        PayloadLimits::default(),
    )
    .unwrap();
    assert_ne!(alternate_source.root(), expected_root);
    let alternate_output = BlockExecutionOutput {
        state: alternate_state.take_bundle(),
        result: alternate_outcome.execution_result.clone(),
    };
    let store = InMemoryBalStore::new(BalConfig::with_in_memory_retention_distance(1));
    store
        .insert(
            fresh.source().native_block,
            RawBal::new(outcome.block_access_list.as_ref().unwrap().as_raw().clone()),
        )
        .unwrap();
    let output = BlockExecutionOutput {
        state: state.take_bundle(),
        result: outcome.execution_result.clone(),
    };
    let provider = BlockchainProvider::new(factory).unwrap();
    let executed =
        ExecutedBlock::new(Arc::new(outcome.block.clone()), Arc::new(output), Default::default());
    provider
        .canonical_in_memory_state()
        .update_chain(NewCanonicalChain::Commit { new: vec![executed.clone()] });
    let historical = SourcePayloadSnapshot::from_reth_historical(
        &provider,
        &store,
        outcome.block.hash(),
        PayloadLimits::default(),
    )
    .unwrap();
    assert_eq!(historical.root(), expected_root);
    assert_eq!(historical.source().native_block, fresh.source().native_block);
    let limits = PayloadLimits { bytes: 363, ..Default::default() };
    assert!(matches!(
        SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &store,
            outcome.block.hash(),
            limits,
        ),
        Err(PayloadSourceError::Payload(PayloadError::LimitExceeded("component bytes")))
    ));
    let limited: Methods = PurethRpc::new(QueryService::from_payload_provider(
        provider.clone(),
        store.clone(),
        limits,
    ))
    .into_rpc()
    .into();
    let (limited_cases, _) = query_cases(&historical);
    assert_eq!(rpc_query(&limited, &limited_cases[0].0).await["error"]["code"], -32602);
    assert_eq!(
        rpc_association(
            &limited,
            &TransactionReceiptRequest { block_hash: outcome.block.hash(), transaction_index: 0 },
        )
        .await["error"]["code"],
        -32602
    );
    for association in [false, true] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let service = QueryService::from_payload_provider(
            provider.clone(),
            CancellingBalStore { cancelled: cancelled.clone(), reads: reads.clone() },
            PayloadLimits::default(),
        );
        let request =
            TransactionReceiptRequest { block_hash: outcome.block.hash(), transaction_index: 0 };
        let result = if association {
            service.transaction_receipt_with_cancel(request, &cancelled).map(|_| ())
        } else {
            let (cases, _) = query_cases(&historical);
            service.query_with_cancel(cases[0].0.clone(), &cancelled).map(|_| ())
        };
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert!(
            matches!(result, Err(QueryError::PayloadAcquisition(PayloadSourceError::Cancelled))),
            "association={association}: {result:?}"
        );
        assert!(matches!(
            SourcePayloadSnapshot::from_reth_historical_with_cancel(
                &provider,
                &store,
                B256::ZERO,
                PayloadLimits::default(),
                &cancelled,
            ),
            Err(PayloadSourceError::Cancelled)
        ));
    }
    for source in [&fresh, &historical] {
        let payload = source.payload();
        for (object, path, operation) in [
            (ComponentObject::Transactions, "[0]", SelectionOperation::Whole {}),
            (ComponentObject::Withdrawals, "[0].amount", SelectionOperation::Value {}),
            (ComponentObject::ExecutionRequests, ".withdrawals", SelectionOperation::Length {}),
            (ComponentObject::BlockAccessList, ".", SelectionOperation::Slice { start: 0, end: 1 }),
            (ComponentObject::ExecutionSummary, ".number", SelectionOperation::Value {}),
        ] {
            let component = ComponentSnapshot::from_payload(payload, object);
            let request = SelectionRequest {
                include_proof: true,
                selections: vec![ReceiptSelection { path: path.into(), operation }],
            };
            let response =
                select_component(component, &request, SelectionLimits::default()).unwrap();
            verify_component_selection(
                object,
                &request,
                &response,
                component.tree().root(),
                SelectionLimits::default(),
            )
            .unwrap();
            if object == ComponentObject::ExecutionSummary {
                assert_eq!(response.selection.root, expected_root);
            } else {
                let link = link_component(component, payload.summary(), SelectionLimits::default())
                    .unwrap();
                verify_component_link(
                    object,
                    component.tree().root(),
                    &link,
                    expected_root,
                    SelectionLimits::default(),
                )
                .unwrap();
            }
        }
        let request = SelectionRequest {
            include_proof: true,
            selections: vec![ReceiptSelection {
                path: "[0].status".into(),
                operation: SelectionOperation::Value {},
            }],
        };
        let receipts = &payload.components().receipts;
        let proof =
            select_receipt_multiproof(receipts, &request, SelectionLimits::default()).unwrap();
        verify_receipt_selection(
            &request,
            &proof,
            payload.summary().summary().receipts_root,
            SelectionLimits::default(),
        )
        .unwrap();
        let summary_request = SelectionRequest {
            include_proof: true,
            selections: vec![ReceiptSelection {
                path: ".receipts_root".into(),
                operation: SelectionOperation::Value {},
            }],
        };
        let summary_proof = select_component(
            ComponentSnapshot::ExecutionSummary(payload.summary()),
            &summary_request,
            SelectionLimits::default(),
        )
        .unwrap();
        verify_component_selection(
            ComponentObject::ExecutionSummary,
            &summary_request,
            &summary_proof,
            expected_root,
            SelectionLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summary_proof.selection.results[0].values_ssz[0].as_ref(),
            proof.root.as_slice()
        );
        assert!(std::ptr::eq(
            ComponentSnapshot::from_payload(payload, ComponentObject::Transactions).tree(),
            payload.components().transactions.tree()
        ));
    }
    let (cases, summary_bytes) = query_cases(&historical);
    let module = assert_query_service(
        QueryService::from_payload_provider(
            provider.clone(),
            store.clone(),
            PayloadLimits::default(),
        ),
        &cases,
        &summary_bytes,
        "canonical",
    )
    .await;
    let association =
        TransactionReceiptRequest { block_hash: outcome.block.hash(), transaction_index: 0 };
    let json = rpc_association(&module, &association).await;
    let response: TransactionReceiptResponse =
        serde_json::from_value(json["result"].clone()).unwrap();
    assert_eq!(response.transaction_hash, *outcome.block.body().transactions[0].tx_hash());
    verify_transaction_receipt(&association, &response, expected_root, SelectionLimits::default())
        .unwrap();
    assert_eq!(store.prune(fresh.source().native_block.number + 2).unwrap(), 1);
    assert!(matches!(
        SourcePayloadSnapshot::from_reth_historical(
            &provider,
            &store,
            outcome.block.hash(),
            PayloadLimits::default()
        ),
        Err(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))
    ));
    assert_eq!(historical.root(), expected_root);
    assert_eq!(rpc_query(&module, &cases[0].0).await["error"]["code"], -32001);
    assert_eq!(rpc_association(&module, &association).await["error"]["code"], -32001);
    let (receipt_request, receipt_root) = &cases[5];
    let json = rpc_query(&module, receipt_request).await;
    let response: QueryResponse = serde_json::from_value(json["result"].clone()).unwrap();
    verify_query_response(receipt_request, &response, *receipt_root, SelectionLimits::default())
        .unwrap();
    store.insert(fresh.source().native_block, RawBal::new(Bytes::from_static(&[0xc0]))).unwrap();
    assert_eq!(rpc_query(&module, &cases[0].0).await["error"]["code"], -32603);
    assert_eq!(rpc_association(&module, &association).await["error"]["code"], -32603);
    store
        .insert(
            alternate_source.source().native_block,
            RawBal::new(alternate_outcome.block_access_list.as_ref().unwrap().as_raw().clone()),
        )
        .unwrap();
    provider.canonical_in_memory_state().update_chain(NewCanonicalChain::Reorg {
        old: vec![executed],
        new: vec![ExecutedBlock::new(
            Arc::new(alternate_outcome.block.clone()),
            Arc::new(alternate_output),
            Default::default(),
        )],
    });
    assert_eq!(rpc_query(&module, &cases[0].0).await["error"]["code"], -32001);
    assert_eq!(rpc_association(&module, &association).await["error"]["code"], -32001);
    let (alternate_cases, alternate_summary_bytes) = query_cases(&alternate_source);
    let second_module = assert_query_service(
        QueryService::from_payload_provider(provider, store, PayloadLimits::default()),
        &alternate_cases,
        &alternate_summary_bytes,
        "canonical",
    )
    .await;
    let (request, root) = &alternate_cases[0];
    let json = rpc_query(&module, request).await;
    let response: QueryResponse = serde_json::from_value(json["result"].clone()).unwrap();
    verify_query_response(request, &response, *root, SelectionLimits::default()).unwrap();
    assert_eq!(rpc_query(&second_module, &cases[0].0).await["error"]["code"], -32001);
    let source = Arc::new(
        alternate_source
            .into_execution_source(
                alternate_outcome.block,
                alternate_outcome.execution_result.receipts,
                spec.as_ref(),
                &ExecutionForkConfig::new(12),
                &AtomicBool::new(false),
            )
            .unwrap(),
    );
    let block = source.block();
    let indices = [1, 2, 3, 4, 0, 1];
    let inputs = indices
        .iter()
        .map(|&index| ConvertReceiptInput {
            receipt: source.native_receipts()[index].clone(),
            tx: Recovered::new_unchecked(
                &block.body().transactions()[index],
                block.senders()[index],
            ),
            gas_used: source.receipt_snapshot().receipts().get(index).unwrap().gas_used(),
            next_log_index: source.native_receipts()[..index].iter().map(|r| r.logs.len()).sum(),
            meta: TransactionMeta {
                tx_hash: *block.body().transactions()[index].hash(),
                index: index as u64,
                block_hash: block.hash(),
                block_number: block.number(),
                timestamp: block.timestamp(),
                base_fee: block.base_fee_per_gas(),
                excess_blob_gas: block.excess_blob_gas(),
            },
        })
        .collect();
    let receipts = ExecutionReceiptConverter::new(
        spec,
        ExecutionForkConfig::new(12),
        SelectionLimits::default(),
    )
    .with_source(source.clone())
    .unwrap()
    .with_receipt_proof(true)
    .convert_receipts_with_block(inputs, block.sealed_block())
    .unwrap();
    assert_eq!(receipts.len(), indices.len());
    assert_eq!(receipts[0], receipts[5]);
    assert!(!receipts[1].inner.status());
    assert_eq!(receipts[1].contract_address, Some(Address::ZERO));
    assert!(receipts[2].inner.status());
    assert_eq!(receipts[2].contract_address, Some(sender.address().create(3)));
    assert!(receipts[3].inner.status());
    assert_eq!(receipts[3].contract_address, None);
    assert_eq!(
        serde_json::to_value(&receipts[3]).unwrap()["authorities"],
        serde_json::json!([
            authority.address(),
            authority.address(),
            authority.address(),
            Address::ZERO
        ])
    );
    for (receipt, index) in receipts.iter().zip(indices) {
        assert_eq!(receipt.transaction_index, Some(index as u64));
        let mut value = serde_json::to_value(receipt).unwrap();
        assert!(value.get("logsBloom").is_none());
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ExecutionRpcReceipt>(value.clone()).unwrap()
            )
            .unwrap(),
            value
        );
        let proof =
            serde_json::from_value(value.as_object_mut().unwrap().remove("sszProof").unwrap())
                .unwrap();
        verify_eip6466_receipt_response(
            &value,
            &proof,
            block.hash(),
            index as u64,
            *block.body().transactions()[index].hash(),
            source.receipt_snapshot().root(),
            SelectionLimits::default(),
        )
        .unwrap();
    }
    for source in [fresh, historical] {
        let status = match source.source().canonicality {
            reth_pureth_receipt::CanonicalityStatus::Canonical => "canonical",
            _ => "unknown",
        };
        let (cases, summary_bytes) = query_cases(&source);
        assert_query_service(QueryService::from_payload(source), &cases, &summary_bytes, status)
            .await;
    }
}

fn assert_runtime_receipt_conversion(
    source: Arc<ExecutionSource>,
    spec: Arc<reth_chainspec::ChainSpec>,
) {
    let block = source.block();
    let limits = SelectionLimits::default();
    let input = || ConvertReceiptInput {
        receipt: source.native_receipts()[0].clone(),
        tx: Recovered::new_unchecked(&block.body().transactions()[0], block.senders()[0]),
        gas_used: source.receipt_snapshot().receipts().get(0).unwrap().gas_used(),
        next_log_index: 0,
        meta: TransactionMeta {
            tx_hash: *block.body().transactions()[0].hash(),
            index: 0,
            block_hash: block.hash(),
            block_number: block.number(),
            base_fee: block.base_fee_per_gas(),
            excess_blob_gas: block.excess_blob_gas(),
            timestamp: block.timestamp(),
        },
    };
    let converter =
        ExecutionReceiptConverter::new(spec.clone(), ExecutionForkConfig::new(12), limits);
    assert!(converter.convert_receipts(vec![input()]).is_err());
    assert!(ExecutionReceiptConverter::new(
        spec.clone(),
        ExecutionForkConfig::new(12),
        SelectionLimits { response_bytes: 1, ..limits }
    )
    .with_source(source.clone())
    .unwrap()
    .convert_receipts_with_block(vec![], block.sealed_block())
    .is_err());
    let converter = converter.with_source(source.clone()).unwrap();
    let receipts =
        converter.convert_receipts_with_block(vec![input()], block.sealed_block()).unwrap();
    let provider = Arc::new(RetainedSources {
        receipts: source.clone(),
        payload: source.clone(),
        receipt_calls: AtomicUsize::new(0),
        payload_calls: AtomicUsize::new(0),
    });
    let operation_converter = converter.clone().with_source_provider(provider.clone());
    assert_eq!(operation_converter.convert_receipts(vec![input()]).unwrap(), receipts);
    assert_eq!(
        operation_converter
            .convert_receipts_with_context(vec![input()], block.sealed_block(), false)
            .unwrap(),
        receipts
    );
    assert!(operation_converter
        .convert_receipts_with_context(vec![], block.sealed_block(), false)
        .unwrap()
        .is_empty());
    assert!(operation_converter
        .convert_receipts_with_context(vec![input()], block.sealed_block(), true)
        .is_err());
    assert_eq!(provider.receipt_calls.load(Ordering::Relaxed), 4);
    assert_eq!(provider.payload_calls.load(Ordering::Relaxed), 0);
    let plain = serde_json::to_value(&receipts[0]).unwrap();
    assert!(plain.get("sszProof").is_none());
    assert!(plain.get("logsBloom").is_none());
    assert_eq!(plain["blockHash"], serde_json::json!(block.hash()));
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<ExecutionRpcReceipt>(plain.clone()).unwrap())
            .unwrap(),
        plain
    );
    let converter = converter.with_receipt_proof(true);
    let receipts = converter.convert_receipts(vec![input(), input()]).unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0], receipts[1]);
    let json = serde_json::to_value(&receipts[0]).unwrap();
    let proof = serde_json::from_value(json["sszProof"].clone()).unwrap();
    let mut bare = json.clone();
    bare.as_object_mut().unwrap().remove("sszProof");
    verify_eip6466_receipt_response(
        &bare,
        &proof,
        block.hash(),
        0,
        input().meta.tx_hash,
        source.receipt_snapshot().root(),
        limits,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<ExecutionRpcReceipt>(json.clone()).unwrap())
            .unwrap(),
        json
    );
    let mut wrong = input();
    wrong.meta.block_hash = B256::ZERO;
    assert!(converter.convert_receipts(vec![wrong]).is_err());
    let mut wrong = input();
    wrong.gas_used += 1;
    assert!(converter.convert_receipts(vec![wrong]).is_err());
    assert!(converter.convert_receipts_with_cancel(vec![input()], &AtomicBool::new(true)).is_err());
    assert!(ExecutionReceiptConverter::new(spec.clone(), ExecutionForkConfig::new(13), limits)
        .with_source(source.clone())
        .is_err());
    assert!(ExecutionReceiptConverter::new(
        spec,
        ExecutionForkConfig::new(12),
        SelectionLimits { response_bytes: 1, ..limits }
    )
    .with_source(source.clone())
    .unwrap()
    .convert_receipts(vec![input()])
    .is_err());
    let header = SealedHeader::new(block.header().clone(), block.hash());
    let mut native_receipt = source.native_receipts()[0].clone();
    let raw_log =
        alloy_primitives::Log::new_unchecked(Address::repeat_byte(0x99), vec![], Bytes::new());
    native_receipt.logs = vec![raw_log.clone(); 6];
    let log = alloy_rpc_types_eth::Log {
        inner: raw_log,
        block_hash: Some(block.hash()),
        block_number: Some(block.number()),
        transaction_hash: Some(input().meta.tx_hash),
        transaction_index: Some(0),
        log_index: Some(25),
        removed: true,
        ..Default::default()
    };
    let local_log =
        converter.convert_log_with_index(log.clone(), &native_receipt, &header, 5).unwrap();
    assert_eq!(local_log.log_index, Some(5));
    assert!(local_log.removed);
    assert!(converter.convert_log_with_index(log.clone(), &native_receipt, &header, 6).is_err());
    assert!(converter.convert_log(log.clone(), &native_receipt, &header).is_err());
    assert!(converter
        .convert_log_with_index(
            log,
            &native_receipt,
            &SealedHeader::new(block.header().clone(), B256::ZERO),
            5
        )
        .is_err());
    let rpc_header = ExecutionHeaderConverter::new(ExecutionForkConfig::new(12))
        .convert_header(header.clone(), Some(42))
        .unwrap();
    assert_eq!(rpc_header.hash, block.hash());
    assert_eq!(rpc_header.size, Some(U256::from(42)));
    assert_eq!(rpc_header.inner.logs_bloom, alloy_primitives::Bloom::repeat_byte(0xff));
    assert!(ExecutionHeaderConverter::new(ExecutionForkConfig::new(13))
        .convert_header(header, None)
        .is_err());
    assert!(ExecutionHeaderConverter::new(ExecutionForkConfig::new(12))
        .convert_header(SealedHeader::new(block.header().clone(), B256::ZERO), None)
        .is_err());
}

fn query_cases(source: &SourcePayloadSnapshot) -> (Vec<(QueryRequest, B256)>, Bytes) {
    let hash = source.source().native_block.hash;
    let payload = source.payload();
    let mut cases = Vec::new();
    for (object, name, path, operation) in [
        (ComponentObject::Transactions, "transactions", "[0]", SelectionOperation::Whole {}),
        (ComponentObject::Withdrawals, "withdrawals", "[0].amount", SelectionOperation::Value {}),
        (
            ComponentObject::ExecutionRequests,
            "execution_requests",
            ".withdrawals",
            SelectionOperation::Length {},
        ),
        (
            ComponentObject::BlockAccessList,
            "block_access_list",
            ".",
            SelectionOperation::Slice { start: 0, end: 1 },
        ),
        (
            ComponentObject::ExecutionSummary,
            "execution_summary",
            ".number",
            SelectionOperation::Value {},
        ),
    ] {
        cases.push((
            QueryRequest {
                block_hash: hash,
                object: name.into(),
                selection: SelectionRequest {
                    include_proof: true,
                    selections: vec![ReceiptSelection { path: path.into(), operation }],
                },
            },
            ComponentSnapshot::from_payload(payload, object).tree().root(),
        ));
    }
    cases.push((
        QueryRequest {
            block_hash: hash,
            object: "receipts".into(),
            selection: SelectionRequest {
                include_proof: true,
                selections: vec![ReceiptSelection {
                    path: "[0].status".into(),
                    operation: SelectionOperation::Value {},
                }],
            },
        },
        payload.components().receipts.root(),
    ));
    let summary_request = QueryRequest {
        block_hash: hash,
        object: "execution_summary".into(),
        selection: SelectionRequest {
            include_proof: true,
            selections: vec![ReceiptSelection {
                path: ".".into(),
                operation: SelectionOperation::Whole {},
            }],
        },
    };
    let summary_bytes = Bytes::copy_from_slice(payload.summary().serialized());
    assert_ne!(summary_bytes.as_ref(), payload.serialized());
    cases.push((summary_request, source.root()));
    let mut repeated = cases[0].0.clone();
    repeated.selection.selections.push(repeated.selection.selections[0].clone());
    cases.push((repeated, cases[0].1));
    for (object, path, operation) in [
        ("execution_payload", ".", SelectionOperation::Whole {}),
        ("transactions", ".", SelectionOperation::Length {}),
        ("transactions", ".", SelectionOperation::Range { start: 0, end: 1 }),
        ("transactions", "[0].payload", SelectionOperation::Variant {}),
        ("transactions", "[0].payload.chain_id", SelectionOperation::Presence {}),
        ("transactions", "[0].payload.input_", SelectionOperation::Slice { start: 0, end: 0 }),
        ("transactions", "[0].signature", SelectionOperation::Slice { start: 31, end: 33 }),
        ("transactions", "[0].payload.value", SelectionOperation::Value {}),
        ("withdrawals", ".", SelectionOperation::Whole {}),
        ("withdrawals", ".", SelectionOperation::Length {}),
        ("withdrawals", ".", SelectionOperation::Range { start: 0, end: 1 }),
        ("withdrawals", "[0].index", SelectionOperation::Value {}),
        ("withdrawals", "[0].validator_index", SelectionOperation::Value {}),
        ("withdrawals", "[0].address", SelectionOperation::Slice { start: 1, end: 3 }),
        ("execution_requests", ".", SelectionOperation::Whole {}),
        ("execution_requests", ".deposits", SelectionOperation::Length {}),
        ("execution_requests", ".withdrawals", SelectionOperation::Range { start: 0, end: 0 }),
        ("execution_requests", ".consolidations", SelectionOperation::Whole {}),
        ("block_access_list", ".", SelectionOperation::Whole {}),
        ("block_access_list", ".", SelectionOperation::Length {}),
        ("block_access_list", ".", SelectionOperation::Range { start: 0, end: 1 }),
        ("execution_summary", ".gas_limits", SelectionOperation::Whole {}),
        ("execution_summary", ".gas_used.blob", SelectionOperation::Value {}),
        ("execution_summary", ".base_fees_per_gas.blob", SelectionOperation::Whole {}),
        ("execution_summary", ".extra_data", SelectionOperation::Slice { start: 1, end: 3 }),
        ("execution_summary", ".excess_gas.regular", SelectionOperation::Value {}),
        ("receipts", "[0]", SelectionOperation::Whole {}),
        ("receipts", ".", SelectionOperation::Length {}),
        ("receipts", ".", SelectionOperation::Range { start: 0, end: 1 }),
        ("receipts", "[0]", SelectionOperation::Variant {}),
        ("receipts", "[0].contract_address", SelectionOperation::Presence {}),
        ("receipts", "[0].logs", SelectionOperation::Length {}),
        ("receipts", "[0].logs", SelectionOperation::Range { start: 0, end: 0 }),
        (
            "receipts",
            ".",
            SelectionOperation::Filter {
                output: FilterOutput::Receipts,
                predicates: vec![FilterPredicate::Status { value: true }],
            },
        ),
        (
            "receipts",
            ".",
            SelectionOperation::Filter {
                output: FilterOutput::Logs,
                predicates: vec![FilterPredicate::LogAddress { address: Address::ZERO }],
            },
        ),
    ] {
        let root = match object {
            "transactions" => payload.components().transactions.root(),
            "withdrawals" => payload.components().withdrawals.root(),
            "execution_requests" => payload.components().requests.root(),
            "block_access_list" => payload.components().block_access_list.root(),
            "receipts" => payload.components().receipts.root(),
            _ => source.root(),
        };
        cases.push((
            QueryRequest {
                block_hash: hash,
                object: object.into(),
                selection: SelectionRequest {
                    include_proof: true,
                    selections: vec![ReceiptSelection { path: path.into(), operation }],
                },
            },
            root,
        ));
    }
    (cases, summary_bytes)
}

async fn assert_query_service<P>(
    mut service: QueryService<P>,
    cases: &[(QueryRequest, B256)],
    summary_bytes: &Bytes,
    status: &str,
) -> Methods
where
    QueryService<P>: QueryHandler + Send + Sync + 'static,
{
    let response = service.query(cases[6].0.clone()).unwrap();
    assert_eq!(response.selection.results[0].values_ssz, vec![summary_bytes.clone()]);
    let response = service.query(cases[7].0.clone()).unwrap();
    assert_eq!(response.selection.results[0], response.selection.results[1]);
    for (request, root) in cases {
        let response = service.query(request.clone()).unwrap();
        assert_eq!(response.block_status, status);
        verify_query_response(request, &response, *root, SelectionLimits::default()).unwrap();
        assert!(verify_query_response(request, &response, B256::ZERO, SelectionLimits::default())
            .is_err());
        for mutation in 0..3 {
            let mut changed = response.clone();
            match mutation {
                0 => changed.selection.multiproof.as_mut().unwrap().targets[0][0] ^= 1,
                1 => changed.selection.multiproof.as_mut().unwrap().helpers.push(B256::ZERO),
                _ => {
                    if let Some(value) = changed.selection.results[0].values_ssz.first_mut() {
                        *value = Bytes::from_static(&[0xff]);
                    } else {
                        changed.selection.results[0].values_ssz.push(Bytes::from_static(&[0xff]));
                    }
                }
            }
            assert!(verify_query_response(request, &changed, *root, SelectionLimits::default())
                .is_err());
        }
        let mut value_only = request.clone();
        value_only.selection.include_proof = false;
        let response = service.query(value_only.clone()).unwrap();
        assert_eq!(response.selection.multiproof, None);
        assert_eq!(
            verify_query_response(&value_only, &response, *root, SelectionLimits::default()),
            Err(ResponseVerificationError::Selection(SelectionError::Unverified))
        );
        assert!(matches!(
            service.query_with_cancel(request.clone(), &AtomicBool::new(true)),
            Err(QueryError::Selection(SelectionError::Cancelled))
        ));
    }
    let request = cases[0].0.clone();
    let mut invalid = request.clone();
    invalid.block_hash = B256::ZERO;
    assert!(matches!(
        service.query(invalid),
        Err(QueryError::Provider(LookupError::UnknownBlock) |
            QueryError::PayloadAcquisition(PayloadSourceError::MissingInput(
                PayloadSourceInput::Block
            )))
    ));
    for object in ["payload", "block", "unknown"] {
        let mut invalid = request.clone();
        invalid.object = object.into();
        assert!(matches!(service.query(invalid), Err(QueryError::UnsupportedObject)));
    }
    let response = service.query(request.clone()).unwrap();
    service = service.with_limits(SelectionLimits {
        response_bytes: serde_json::to_vec(&response).unwrap().len() - 1,
        ..Default::default()
    });
    assert!(matches!(
        service.query(request.clone()),
        Err(QueryError::Selection(SelectionError::LimitExceeded("response bytes")))
    ));
    service = service.with_limits(SelectionLimits {
        request_bytes: serde_json::to_vec(&request).unwrap().len() - 1,
        ..Default::default()
    });
    assert!(matches!(
        service.query(request.clone()),
        Err(QueryError::Selection(SelectionError::LimitExceeded("request bytes")))
    ));
    let module: Methods =
        PurethRpc::new(service.with_limits(SelectionLimits::default())).into_rpc().into();
    let association =
        TransactionReceiptRequest { block_hash: cases[0].0.block_hash, transaction_index: 0 };
    let json = rpc_association(&module, &association).await;
    let response: TransactionReceiptResponse =
        serde_json::from_value(json["result"].clone()).unwrap();
    verify_transaction_receipt(&association, &response, cases[6].1, SelectionLimits::default())
        .unwrap();
    assert_eq!(response.block_status, status);
    let positional = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "pureth_transactionReceipt",
        "params": [association],
    });
    let (positional, _) = module.raw_json_request(&positional.to_string(), 1).await.unwrap();
    let positional: serde_json::Value = serde_json::from_str(positional.get()).unwrap();
    let positional: TransactionReceiptResponse =
        serde_json::from_value(positional["result"].clone()).unwrap();
    verify_transaction_receipt(&association, &positional, cases[6].1, SelectionLimits::default())
        .unwrap();
    assert_eq!(positional, response);
    let mut missing_association = association.clone();
    missing_association.block_hash = B256::ZERO;
    assert_eq!(rpc_association(&module, &missing_association).await["error"]["code"], -32001);
    let mut wrong_index = association;
    wrong_index.transaction_index = u64::MAX;
    assert_eq!(rpc_association(&module, &wrong_index).await["error"]["code"], -32602);
    let mut missing = request;
    missing.block_hash = B256::ZERO;
    let json = rpc_query(&module, &missing).await;
    assert_eq!(json["error"]["code"], -32001);
    for (request, root) in cases {
        let json = rpc_query(&module, request).await;
        let response: QueryResponse = serde_json::from_value(json["result"].clone()).unwrap();
        verify_query_response(request, &response, *root, SelectionLimits::default()).unwrap();
        assert_eq!(response.block_status, status);
    }
    module
}

async fn rpc_query(module: &Methods, request: &QueryRequest) -> serde_json::Value {
    let raw = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "pureth_query", "params": { "request": request } });
    let (response, _) = module.raw_json_request(&raw.to_string(), 1).await.unwrap();
    serde_json::from_str(response.get()).unwrap()
}

async fn rpc_association(
    module: &Methods,
    request: &TransactionReceiptRequest,
) -> serde_json::Value {
    let raw = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "pureth_transactionReceipt", "params": { "request": request } });
    let (response, _) = module.raw_json_request(&raw.to_string(), 1).await.unwrap();
    serde_json::from_str(response.get()).unwrap()
}
