use crate::{
    parse_path, prove_receipt_log_address, receipt_log_address_gindex, resolve,
    verify_receipt_log_address, EnvelopeError, GindexError, ParseError, ProofAccessError,
    ResolvedPath, UnsupportedPath, SCHEMA_ID,
};
use alloy_primitives::{Bytes, B256};
use reth_pureth_receipt::{
    CanonicalityStatus, DeterministicProvider, LookupError, ObjectKind, ProviderBuildError,
    ProviderSnapshot, RootContext,
};
use serde::{Deserialize, Serialize};

const PROOF_FORMAT: &str = "merkle_branch_v0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub block_hash: B256,
    pub object: String,
    pub schema_id: String,
    pub path: String,
    pub include_proof: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResponse {
    pub value_ssz: Bytes,
    pub path: String,
    pub gindex: String,
    pub proof: Vec<B256>,
    pub proof_format: String,
    pub schema_id: String,
    pub root: B256,
    pub object: String,
    pub block_hash: B256,
    pub root_context: String,
    pub producer_revision: String,
    pub block_status: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueryError {
    UnsupportedObject,
    UnsupportedSchema,
    ProofRequired,
    InvalidPath(ParseError),
    UnsupportedPath(UnsupportedPath),
    Provider(LookupError),
    Proof(ProofAccessError),
    InvalidResponse(ResponseVerificationError),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResponseVerificationError {
    ContextMismatch,
    WrongProofFormat,
    InvalidPath(ParseError),
    UnsupportedPath(UnsupportedPath),
    InvalidGindex(GindexError),
    WrongGindex,
    InvalidProof(EnvelopeError),
}

#[derive(Debug)]
pub struct QueryService {
    provider: DeterministicProvider,
}

impl QueryService {
    pub fn new() -> Result<Self, ProviderBuildError> {
        Ok(Self { provider: DeterministicProvider::new()? })
    }

    pub fn query(&self, request: QueryRequest) -> Result<QueryResponse, QueryError> {
        let resolved = validate_request(&request)?;
        let result = self
            .provider
            .lookup(request.block_hash, ObjectKind::Receipts, &request.schema_id)
            .map_err(QueryError::Provider)?;
        query_validated(request, resolved, result)
    }
}

pub fn query_snapshot(
    request: QueryRequest,
    snapshot: &ProviderSnapshot,
) -> Result<QueryResponse, QueryError> {
    let resolved = validate_request(&request)?;
    if request.block_hash != snapshot.block_hash() {
        return Err(QueryError::Provider(LookupError::UnknownBlock));
    }
    if request.object != object_name(snapshot.object()) {
        return Err(QueryError::Provider(LookupError::UnsupportedObject));
    }
    if request.schema_id != snapshot.schema_id() {
        return Err(QueryError::Provider(LookupError::UnsupportedSchema));
    }
    query_validated(request, resolved, snapshot)
}

fn validate_request(request: &QueryRequest) -> Result<ResolvedPath, QueryError> {
    if request.object != "receipts" {
        return Err(QueryError::UnsupportedObject);
    }
    if request.schema_id != SCHEMA_ID {
        return Err(QueryError::UnsupportedSchema);
    }
    if !request.include_proof {
        return Err(QueryError::ProofRequired);
    }

    let tokens = parse_path(&request.path).map_err(QueryError::InvalidPath)?;
    resolve(&tokens).map_err(QueryError::UnsupportedPath)
}

fn query_validated(
    request: QueryRequest,
    resolved: ResolvedPath,
    snapshot: &ProviderSnapshot,
) -> Result<QueryResponse, QueryError> {
    let proof = prove_receipt_log_address(snapshot.receipt_snapshot(), resolved)
        .map_err(QueryError::Proof)?;
    let response = QueryResponse {
        value_ssz: Bytes::copy_from_slice(proof.address.as_slice()),
        path: request.path.clone(),
        gindex: proof.gindex.to_string(),
        proof: proof.branch,
        proof_format: PROOF_FORMAT.to_owned(),
        schema_id: snapshot.schema_id().to_owned(),
        root: snapshot.root(),
        object: object_name(snapshot.object()).to_owned(),
        block_hash: snapshot.block_hash(),
        root_context: root_context_name(snapshot.root_context()).to_owned(),
        producer_revision: snapshot.producer_revision().to_owned(),
        block_status: block_status_name(snapshot.block_status()).to_owned(),
    };
    verify_query_response(&request, &response, snapshot).map_err(QueryError::InvalidResponse)?;
    Ok(response)
}

pub fn verify_query_response(
    request: &QueryRequest,
    response: &QueryResponse,
    expected: &ProviderSnapshot,
) -> Result<(), ResponseVerificationError> {
    if !request.include_proof ||
        request.block_hash != expected.block_hash() ||
        request.object != object_name(expected.object()) ||
        request.schema_id != expected.schema_id() ||
        response.block_hash != expected.block_hash() ||
        response.object != object_name(expected.object()) ||
        response.schema_id != expected.schema_id() ||
        response.path != request.path ||
        response.root != expected.root() ||
        response.root_context != root_context_name(expected.root_context()) ||
        response.producer_revision != expected.producer_revision() ||
        response.block_status != block_status_name(expected.block_status())
    {
        return Err(ResponseVerificationError::ContextMismatch);
    }
    if response.proof_format != PROOF_FORMAT {
        return Err(ResponseVerificationError::WrongProofFormat);
    }

    let tokens = parse_path(&response.path).map_err(ResponseVerificationError::InvalidPath)?;
    let resolved: ResolvedPath =
        resolve(&tokens).map_err(ResponseVerificationError::UnsupportedPath)?;
    let gindex =
        receipt_log_address_gindex(resolved).map_err(ResponseVerificationError::InvalidGindex)?;
    if response.gindex != gindex.to_string() {
        return Err(ResponseVerificationError::WrongGindex);
    }

    verify_receipt_log_address(
        &response.schema_id,
        &response.path,
        response.value_ssz.as_ref(),
        &response.proof,
        response.root,
    )
    .map_err(ResponseVerificationError::InvalidProof)
}

const fn object_name(object: ObjectKind) -> &'static str {
    match object {
        ObjectKind::Receipts => "receipts",
        ObjectKind::Withdrawals => "withdrawals",
    }
}

const fn root_context_name(context: RootContext) -> &'static str {
    match context {
        RootContext::DeterministicTestData => "deterministic_test_data",
        RootContext::RethExperimentalUnanchored => "reth_experimental_unanchored",
    }
}

const fn block_status_name(status: CanonicalityStatus) -> &'static str {
    match status {
        CanonicalityStatus::Canonical => "canonical",
        CanonicalityStatus::NonCanonical => "noncanonical",
        CanonicalityStatus::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{TxLegacy, TxType};
    use alloy_primitives::{Address, Log, Signature, TxKind};
    use reth_ethereum_primitives::{
        Block, BlockBody, Receipt, Transaction as EthereumTransaction, TransactionSigned,
    };
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, DBProvider, DatabaseProviderFactory, ExecutionOutcome, OriginalValuesKnown,
        StateWriteConfig, StateWriter,
    };
    use reth_pureth_receipt::{
        MULTIPLE_LOGS_BLOCK_HASH, PROGRESSIVE_RECEIPTS_BLOCK_HASH, SINGLETON_BLOCK_HASH,
    };

    fn request(block_hash: B256, path: &str) -> QueryRequest {
        QueryRequest {
            block_hash,
            object: "receipts".to_owned(),
            schema_id: SCHEMA_ID.to_owned(),
            path: path.to_owned(),
            include_proof: true,
        }
    }

    fn historical_provider() -> (BlockchainProvider<MockNodeTypesWithDB>, B256) {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        let transaction = TransactionSigned::new_unhashed(
            EthereumTransaction::Legacy(TxLegacy {
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            }),
            Signature::test_signature(),
        );
        let block = RecoveredBlock::try_new_unhashed(
            Block {
                header: Default::default(),
                body: BlockBody { transactions: vec![transaction], ..Default::default() },
            },
            vec![Address::repeat_byte(0x66)],
        )
        .unwrap();
        let block_hash = block.hash();
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21_000,
            logs: vec![Log::new_unchecked(
                Address::repeat_byte(0x11),
                vec![B256::repeat_byte(0x22)],
                Bytes::from_static(&[1, 2, 3]),
            )],
        };
        provider.insert_block(&block).unwrap();
        provider
            .write_state(
                &ExecutionOutcome {
                    first_block: 0,
                    receipts: vec![vec![receipt]],
                    ..Default::default()
                },
                OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        provider.commit().unwrap();

        (BlockchainProvider::new(factory).unwrap(), block_hash)
    }

    #[test]
    fn frozen_provider_cases_return_verified_responses() {
        let service = QueryService::new().unwrap();

        for (block_hash, path, address, gindex, root) in [
            (
                SINGLETON_BLOCK_HASH,
                "[0].logs[0].address",
                0x11,
                "576",
                alloy_primitives::b256!(
                    "5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
                ),
            ),
            (
                MULTIPLE_LOGS_BLOCK_HASH,
                "[0].logs[1].address",
                0x33,
                "4640",
                alloy_primitives::b256!(
                    "d4e90213f6f7fa76997b8d2a3e56c1df70c7846a06e8deeead604844b0cfa43a"
                ),
            ),
            (
                PROGRESSIVE_RECEIPTS_BLOCK_HASH,
                "[5].logs[0].address",
                0x16,
                "45120",
                alloy_primitives::b256!(
                    "a8d13e4ec4c2b516ebd5b536f94784667c0098c5e1d6017453313cea532c1830"
                ),
            ),
        ] {
            let response = service.query(request(block_hash, path)).unwrap();

            assert_eq!(response.value_ssz.as_ref(), &[address; 20]);
            assert_eq!(response.path, path);
            assert_eq!(response.gindex, gindex);
            assert_eq!(response.root, root);
            assert_eq!(response.proof_format, PROOF_FORMAT);
            assert_eq!(response.schema_id, SCHEMA_ID);
            assert_eq!(response.object, "receipts");
            assert_eq!(response.block_hash, block_hash);
            assert_eq!(response.root_context, "deterministic_test_data");
            assert_eq!(response.producer_revision, "deterministic-receipt-provider-v0");
            assert_eq!(response.block_status, "unknown");
            verify_receipt_log_address(
                &response.schema_id,
                &response.path,
                response.value_ssz.as_ref(),
                &response.proof,
                response.root,
            )
            .unwrap();
        }
    }

    #[test]
    fn deterministic_and_supplied_snapshot_paths_match() {
        let service = QueryService::new().unwrap();

        for (block_hash, path) in [
            (SINGLETON_BLOCK_HASH, "[0].logs[0].address"),
            (MULTIPLE_LOGS_BLOCK_HASH, "[0].logs[1].address"),
            (PROGRESSIVE_RECEIPTS_BLOCK_HASH, "[5].logs[0].address"),
        ] {
            let request = request(block_hash, path);
            let snapshot =
                service.provider.lookup(block_hash, ObjectKind::Receipts, SCHEMA_ID).unwrap();

            assert_eq!(service.query(request.clone()), query_snapshot(request, snapshot));
        }
    }

    #[test]
    fn reth_historical_snapshot_returns_a_verified_query_response() {
        let (provider, block_hash) = historical_provider();
        assert_eq!(
            block_hash,
            alloy_primitives::b256!(
                "78dec18c6d7da925bbe773c315653cdc70f6444ed6c1de9ac30bdb36cff74c3b"
            )
        );
        let snapshot = ProviderSnapshot::from_reth_historical(&provider, block_hash).unwrap();
        let request = request(block_hash, "[0].logs[0].address");
        let response = query_snapshot(request.clone(), &snapshot).unwrap();

        assert_eq!(response.value_ssz.as_ref(), &[0x11; 20]);
        assert_eq!(response.gindex, "576");
        assert_eq!(
            response.proof,
            [
                alloy_primitives::b256!(
                    "a48111926253919152be5c26543604338b03a25791b6c6f4444bfdafacc5f771"
                ),
                alloy_primitives::b256!(
                    "37cb3fcfce36a2c4aed31728b918c2572b0dd37ca5f38c8dc042f717a4624fa0"
                ),
                B256::ZERO,
                alloy_primitives::b256!(
                    "0100000000000000000000000000000000000000000000000000000000000000"
                ),
                B256::ZERO,
                alloy_primitives::b256!(
                    "f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"
                ),
                alloy_primitives::b256!(
                    "046cc3ec2d1bdde5d07df87754e5c8eb35ec226dd3aa569fe908394bd2c88a12"
                ),
                B256::ZERO,
                alloy_primitives::b256!(
                    "0100000000000000000000000000000000000000000000000000000000000000"
                ),
            ]
        );
        assert_eq!(
            response.root,
            alloy_primitives::b256!(
                "5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
            )
        );
        assert_eq!(response.root_context, "reth_experimental_unanchored");
        assert_eq!(response.producer_revision, "reth-historical-receipt-provider-v0");
        assert_eq!(response.block_status, "canonical");
        verify_query_response(&request, &response, &snapshot).unwrap();

        let mut changed = response.clone();
        changed.proof[0][0] ^= 1;
        assert!(verify_query_response(&request, &changed, &snapshot).is_err());

        let mut changed = response;
        changed.root[0] ^= 1;
        assert!(verify_query_response(&request, &changed, &snapshot).is_err());
    }

    #[test]
    fn supplied_snapshot_rejects_context_mismatches() {
        let service = QueryService::new().unwrap();
        let snapshot =
            service.provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Receipts, SCHEMA_ID).unwrap();

        assert_eq!(
            query_snapshot(request(B256::repeat_byte(0xff), "[0].logs[0].address"), snapshot),
            Err(QueryError::Provider(LookupError::UnknownBlock))
        );

        let mut invalid = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
        invalid.object = "withdrawals".to_owned();
        assert_eq!(query_snapshot(invalid, snapshot), Err(QueryError::UnsupportedObject));

        let mut invalid = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
        invalid.schema_id = "other-schema".to_owned();
        assert_eq!(query_snapshot(invalid, snapshot), Err(QueryError::UnsupportedSchema));
    }

    #[test]
    fn request_failures_remain_distinct() {
        let service = QueryService::new().unwrap();

        let mut invalid = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
        invalid.object = "withdrawals".to_owned();
        assert_eq!(service.query(invalid), Err(QueryError::UnsupportedObject));

        let mut invalid = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
        invalid.schema_id = "other-schema".to_owned();
        assert_eq!(service.query(invalid), Err(QueryError::UnsupportedSchema));

        let mut invalid = request(SINGLETON_BLOCK_HASH, "[0].logs[0].address");
        invalid.include_proof = false;
        assert_eq!(service.query(invalid), Err(QueryError::ProofRequired));

        assert!(matches!(
            service.query(request(SINGLETON_BLOCK_HASH, "[0")),
            Err(QueryError::InvalidPath(_))
        ));
        assert!(matches!(
            service.query(request(SINGLETON_BLOCK_HASH, "[0].status")),
            Err(QueryError::UnsupportedPath(_))
        ));
        assert!(matches!(
            service.query(request(SINGLETON_BLOCK_HASH, "[1].logs[0].address")),
            Err(QueryError::Proof(ProofAccessError::Resolution(_)))
        ));
        assert_eq!(
            service.query(request(B256::repeat_byte(0xff), "[0].logs[0].address")),
            Err(QueryError::Provider(LookupError::UnknownBlock))
        );
    }

    #[test]
    fn response_verifier_rejects_proof_and_context_mutations() {
        let service = QueryService::new().unwrap();
        let request = request(PROGRESSIVE_RECEIPTS_BLOCK_HASH, "[5].logs[0].address");
        let response = service.query(request.clone()).unwrap();
        let expected = service
            .provider
            .lookup(PROGRESSIVE_RECEIPTS_BLOCK_HASH, ObjectKind::Receipts, SCHEMA_ID)
            .unwrap();

        let mutations: [fn(&mut QueryResponse); 14] = [
            |response| {
                let mut value = response.value_ssz.to_vec();
                value[0] ^= 1;
                response.value_ssz = value.into();
            },
            |response| response.proof[0][0] ^= 1,
            |response| response.proof.reverse(),
            |response| {
                response.proof.pop();
            },
            |response| response.gindex = "576".to_owned(),
            |response| response.root[0] ^= 1,
            |response| response.path = "[0].logs[0].address".to_owned(),
            |response| response.schema_id = "other-schema".to_owned(),
            |response| response.object = "withdrawals".to_owned(),
            |response| response.block_hash = B256::repeat_byte(0xff),
            |response| response.root_context = "reth_experimental_unanchored".to_owned(),
            |response| response.producer_revision = "other-revision".to_owned(),
            |response| response.block_status = "canonical".to_owned(),
            |response| response.proof_format = "other-format".to_owned(),
        ];

        for mutate in mutations {
            let mut changed = response.clone();
            mutate(&mut changed);
            assert!(verify_query_response(&request, &changed, expected).is_err());
        }
    }

    #[test]
    fn response_uses_decimal_string_gindex_and_hex_values() {
        let response = QueryService::new()
            .unwrap()
            .query(request(SINGLETON_BLOCK_HASH, "[0].logs[0].address"))
            .unwrap();
        let json = serde_json::to_value(response).unwrap();

        assert_eq!(json["gindex"], "576");
        assert_eq!(json["value_ssz"], format!("0x{}", "11".repeat(20)));
        assert_eq!(json["proof"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn request_requires_every_field() {
        let request = serde_json::json!({
            "block_hash": SINGLETON_BLOCK_HASH,
            "object": "receipts",
            "schema_id": "pureth-receipt-v0",
            "path": "[0].logs[0].address",
            "include_proof": true
        });

        assert!(serde_json::from_value::<QueryRequest>(request.clone()).is_ok());
        for field in ["block_hash", "object", "schema_id", "path", "include_proof"] {
            let mut incomplete = request.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<QueryRequest>(incomplete).is_err(), "{field}");
        }
    }
}
