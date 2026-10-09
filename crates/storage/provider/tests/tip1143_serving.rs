//! T-023: the real snap network request handler serves reconstructed original bytes.
//! Wire omission on provider failure follows the existing snap contract; internal
//! reads must still return a node error, never successful empty bytecode.

use alloy_consensus::Header;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{keccak256, Address, Bytes, B256};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_eth_wire_types::{
    snap::{ByteCodesMessage, GetByteCodesMessage, SnapProtocolMessage},
    EthNetworkPrimitives,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_network::eth_requests::{EthRequestHandler, IncomingEthRequest};
use reth_network_api::{noop::NoopNetwork, test_utils::PeersHandle};
use reth_network_p2p::snap::client::SnapResponse;
use reth_primitives_traits::{Account, SealedHeader};
use reth_provider::{
    providers::BlockchainProvider,
    test_utils::{create_test_provider_factory, insert_headers},
    ChainSpecProvider, ProviderError, StateProviderFactory,
};
use reth_rpc::EthApiBuilder;
use reth_rpc_eth_api::EthApiServer;
use reth_storage_api::{AccountReader, BytecodeReader, DBProvider, ValidatedCode};
use reth_transaction_pool::test_utils::testing_pool;
use tokio::sync::{mpsc, oneshot};

#[tokio::test]
async fn t023_network_serves_original_bytes_and_omits_corrupt_code() {
    let factory = create_test_provider_factory();
    insert_headers(&factory, &[SealedHeader::new(Header::default(), B256::ZERO)]);
    let legacy = Bytes::from_static(&[0x60]);
    let mut multi = vec![0; 49083];
    multi[0] = 0x60;
    multi[1] = 0x23;
    multi[24541] = 0x60;
    multi[24542] = 0x24;
    multi[49082] = 0x5b;
    let multi = Bytes::from(multi);
    let originals = [legacy, multi];
    let writer = factory.provider_rw().unwrap();
    for (i, bytes) in originals.iter().enumerate() {
        writer
            .write_chunked_code(
                Address::with_last_byte(i as u8 + 1),
                Account { bytecode_hash: Some(keccak256(bytes)), ..Default::default() },
                &ValidatedCode::new(bytes.clone()).unwrap(),
            )
            .unwrap();
    }
    writer.commit().unwrap();
    let client = BlockchainProvider::new(factory.clone()).unwrap();
    let (peers, _peer_commands) = mpsc::unbounded_channel();
    let (requests, incoming) = mpsc::channel(2);
    let handler = EthRequestHandler::<_, EthNetworkPrimitives>::new(
        client,
        PeersHandle::new(peers),
        incoming,
    );
    let task = tokio::spawn(handler);
    for (id, bytes) in originals.iter().enumerate() {
        let (response, received) = oneshot::channel();
        requests
            .send(IncomingEthRequest::GetSnap {
                peer_id: Default::default(),
                request: SnapProtocolMessage::GetByteCodes(GetByteCodesMessage {
                    request_id: id as u64,
                    hashes: vec![keccak256(bytes)],
                    response_bytes: 1_000_000,
                }),
                response,
            })
            .await
            .unwrap();
        assert_eq!(
            received.await.unwrap().unwrap(),
            SnapResponse::ByteCodes(ByteCodesMessage {
                request_id: id as u64,
                codes: vec![bytes.clone()],
            })
        );
    }
    let hash = keccak256(&originals[1]);
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
    writer
        .tx_ref()
        .delete::<tables::BytecodeChunks>(keccak256(&originals[1][24541..49082]), None)
        .unwrap();
    writer.commit().unwrap();
    assert!(factory.latest().unwrap().bytecode_by_hash(&hash).is_err());
    let (response, received) = oneshot::channel();
    requests
        .send(IncomingEthRequest::GetSnap {
            peer_id: Default::default(),
            request: SnapProtocolMessage::GetByteCodes(GetByteCodesMessage {
                request_id: 3,
                hashes: vec![hash],
                response_bytes: 1_000_000,
            }),
            response,
        })
        .await
        .unwrap();
    // No result entry is different from an entry containing empty runtime bytes.
    assert_eq!(
        received.await.unwrap().unwrap(),
        SnapResponse::ByteCodes(ByteCodesMessage { request_id: 3, codes: vec![] })
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

/// T-023: invoke the real eth_getCode server method over persisted state.
/// Pool/network stand-ins are unrelated to code retrieval; the state provider is real.
#[tokio::test(flavor = "multi_thread")]
async fn t023_rpc_server_returns_original_code_and_propagates_reconstruction_failure() {
    let factory = create_test_provider_factory();
    insert_headers(&factory, &[SealedHeader::new(Header::default(), B256::ZERO)]);
    let originals = [Bytes::from_static(&[0x60]), Bytes::from(vec![0; 24542])];
    let writer = factory.provider_rw().unwrap();
    for (index, bytes) in originals.iter().enumerate() {
        writer
            .write_chunked_code(
                Address::with_last_byte(index as u8 + 1),
                Account { bytecode_hash: Some(keccak256(bytes)), ..Default::default() },
                &ValidatedCode::new(bytes.clone()).unwrap(),
            )
            .unwrap();
    }
    writer.commit().unwrap();
    let client = BlockchainProvider::new(factory.clone()).unwrap();
    let api = EthApiBuilder::new(
        client.clone(),
        testing_pool(),
        NoopNetwork::default(),
        EthEvmConfig::new(client.chain_spec()),
    )
    .build();
    for (index, expected) in originals.iter().enumerate() {
        let result = EthApiServer::get_code(
            &api,
            Address::with_last_byte(index as u8 + 1),
            Some(BlockId::Number(BlockNumberOrTag::Latest)),
        )
        .await
        .unwrap();
        assert_eq!(&result, expected);
        assert_eq!(keccak256(&result), keccak256(expected));
    }
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::BytecodeChunks>(keccak256([0]), None).unwrap());
    writer.commit().unwrap();
    assert!(factory.latest().unwrap().bytecode_by_hash(&keccak256(&originals[1])).is_err());
    // Rebuild the server to avoid a previously authenticated cached response.
    drop(api);
    let api = EthApiBuilder::new(
        client.clone(),
        testing_pool(),
        NoopNetwork::default(),
        EthEvmConfig::new(client.chain_spec()),
    )
    .build();
    let error = EthApiServer::get_code(
        &api,
        Address::with_last_byte(2),
        Some(BlockId::Number(BlockNumberOrTag::Latest)),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), -32603);
    assert_eq!(
        EthApiServer::get_code(
            &api,
            Address::with_last_byte(1),
            Some(BlockId::Number(BlockNumberOrTag::Latest))
        )
        .await
        .unwrap(),
        originals[0]
    );
}

/// T-023: absent descriptors must not turn surviving contracts into empty RPC code.
#[tokio::test(flavor = "multi_thread")]
async fn t023_rpc_missing_descriptor_is_a_node_error() {
    assert_rpc_required_code_failure(0).await;
}

/// T-023: required payload failures propagate through the actual account RPC path.
#[tokio::test(flavor = "multi_thread")]
async fn t023_rpc_missing_payload_is_a_node_error() {
    assert_rpc_required_code_failure(1).await;
}

/// T-023: legacy missing rows have the same required-account obligation.
#[tokio::test(flavor = "multi_thread")]
async fn t023_rpc_missing_legacy_row_is_a_node_error() {
    assert_rpc_required_code_failure(2).await;
}

async fn assert_rpc_required_code_failure(mode: u8) {
    let factory = create_test_provider_factory();
    insert_headers(&factory, &[SealedHeader::new(Header::default(), B256::ZERO)]);
    let original =
        if mode == 2 { Bytes::from_static(&[0x60]) } else { Bytes::from(vec![0; 24542]) };
    let hash = keccak256(&original);
    let owner = Address::with_last_byte(23);
    let account = Account { bytecode_hash: Some(hash), ..Default::default() };
    let code = ValidatedCode::new(original.clone()).unwrap();
    let writer = factory.provider_rw().unwrap();
    writer.write_chunked_code(owner, account.clone(), &code).unwrap();
    writer
        .tx_ref()
        .put::<tables::PlainAccountState>(Address::with_last_byte(24), Account::default())
        .unwrap();
    writer
        .tx_ref()
        .put::<tables::PlainAccountState>(
            Address::with_last_byte(25),
            Account { bytecode_hash: Some(keccak256([])), ..Default::default() },
        )
        .unwrap();
    writer.commit().unwrap();
    for phase in 0..3 {
        if phase == 1 {
            let writer = factory.provider_rw().unwrap();
            match mode {
                0 => assert!(writer
                    .tx_ref()
                    .delete::<tables::BytecodeChunkDescriptors>(hash, None)
                    .unwrap()),
                1 => assert!(writer
                    .tx_ref()
                    .delete::<tables::BytecodeChunks>(keccak256([0]), None)
                    .unwrap()),
                _ => assert!(writer.tx_ref().delete::<tables::Bytecodes>(hash, None).unwrap()),
            }
            writer.commit().unwrap();
            let reader = factory.provider().unwrap();
            assert_eq!(
                reader.tx_ref().get::<tables::PlainAccountState>(owner).unwrap(),
                Some(account.clone())
            );
            assert_eq!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap(), None);
            if mode == 0 {
                assert_eq!(
                    reader.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap(),
                    None
                );
            } else if mode == 1 {
                assert_eq!(
                    reader.tx_ref().get::<tables::BytecodeChunks>(keccak256([0])).unwrap(),
                    None
                );
            }
        } else if phase == 2 {
            let writer = factory.provider_rw().unwrap();
            writer.write_chunked_code(owner, account.clone(), &code).unwrap();
            writer.commit().unwrap();
        }
        // Each phase creates both a new blockchain provider and a new server/cache.
        let client = BlockchainProvider::new(factory.clone()).unwrap();
        let api = EthApiBuilder::new(
            client.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(client.chain_spec()),
        )
        .build();
        for empty in [Address::ZERO, Address::with_last_byte(24), Address::with_last_byte(25)] {
            assert_eq!(
                EthApiServer::get_code(&api, empty, Some(BlockId::latest())).await.unwrap(),
                Bytes::new()
            );
        }
        let response = EthApiServer::get_code(&api, owner, Some(BlockId::latest())).await;
        if phase == 1 {
            // Invoke RPC before the internal assertion so this regression exercises both paths.
            let internal = factory.latest().unwrap().account_code(&owner);
            let error = response
                .expect_err("missing required code must never be successful empty RPC bytes");
            assert_eq!(error.code(), -32603);
            assert_eq!(error.data().map(|data| data.get()), None);
            let ProviderError::CodeChunk(internal) = internal.unwrap_err() else {
                panic!("expected structured required-code error")
            };
            assert_eq!(internal.code_hash, hash);
            assert_eq!(internal.index, if mode == 1 { 1 } else { 0 });
            assert_eq!(internal.expected_length, if mode == 1 { Some(1) } else { None });
            assert_eq!(internal.context, None);
            assert_eq!(
                format!("{:?}", internal.reason),
                if mode == 1 { "MissingPayload" } else { "MissingCode" }
            );
            assert_eq!(
                error.message(),
                if mode == 1 {
                    format!("code {hash} chunk 1, expected length Some(1): MissingPayload")
                } else {
                    format!("code {hash} chunk 0, expected length None: MissingCode")
                }
            );
        } else {
            assert_eq!(response.unwrap(), original);
            assert_eq!(
                factory.latest().unwrap().account_code(&owner).unwrap().unwrap().original_bytes(),
                original
            );
        }
    }
}
