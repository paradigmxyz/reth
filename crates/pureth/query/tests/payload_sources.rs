#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![cfg(not(target_arch = "wasm32"))]

use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_eips::{
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
    eip4895::Withdrawal,
    eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE},
};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_chainspec::ChainSpecBuilder;
use reth_db_common::init::init_genesis;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{execute::BlockBuilder, ConfigureEvm, NextBlockEnvAttributes};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{Recovered, SealedHeader};
use reth_provider::{
    providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    BalConfig, BalStore, BlockExecutionOutput, EvmStateProviderAdapter, InMemoryBalStore, RawBal,
};
use reth_pureth_block::{
    PayloadLimits, PayloadSourceError, PayloadSourceInput, SourcePayloadSnapshot,
};
use reth_pureth_query::{
    link_component, select_component, select_receipt_multiproof, verify_component_link,
    verify_component_selection, verify_receipt_selection, ComponentObject, ComponentSnapshot,
    ReceiptSelection, SelectionLimits, SelectionOperation, SelectionRequest,
};
use reth_revm::database::StateProviderDatabase;
use revm::database::State;
use std::sync::Arc;

#[test]
fn real_builder_and_captured_payloads_feed_original_component_queries_and_links() {
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
    provider.canonical_in_memory_state().update_chain(NewCanonicalChain::Commit {
        new: vec![ExecutedBlock::new(
            Arc::new(outcome.block.clone()),
            Arc::new(output),
            Default::default(),
        )],
    });
    let historical = SourcePayloadSnapshot::from_reth_historical(
        &provider,
        &store,
        outcome.block.hash(),
        PayloadLimits::default(),
    )
    .unwrap();
    assert_eq!(historical.root(), expected_root);
    assert_eq!(historical.source().native_block, fresh.source().native_block);
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
}
