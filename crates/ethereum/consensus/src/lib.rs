//! Beacon consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{fmt::Debug, sync::Arc};
use alloy_consensus::{constants::MAXIMUM_EXTRA_DATA_SIZE, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::B256;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::{
    Consensus, ConsensusError, FullConsensus, HeaderValidator, ReceiptRootBloom, TransactionRoot,
};
use reth_consensus_common::validation::{
    validate_4844_header_standalone, validate_against_parent_4844,
    validate_against_parent_eip1559_base_fee, validate_against_parent_gas_limit,
    validate_against_parent_hash_number, validate_against_parent_timestamp,
    validate_block_pre_execution, validate_block_pre_execution_with_tx_root,
    validate_body_against_header, validate_header_base_fee, validate_header_extra_data,
    validate_header_gas,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{
    AlloyBlockHeader, Block, BlockBody, BlockHeader, NodePrimitives, RecoveredBlock, SealedBlock,
    SealedHeader,
};

#[cfg(any(feature = "std", test))]
use alloy_primitives::Bytes;

#[cfg(feature = "std")]
use alloc::boxed::Box;
#[cfg(feature = "std")]
use core::ops::RangeInclusive;
#[cfg(feature = "std")]
use reth_consensus::SupplementalDataFuture;
#[cfg(feature = "std")]
use reth_storage_api::{DependencyProofRequest, DependencyProofStore};

mod validation;
pub use validation::{
    compare_receipts_root_and_logs_bloom, validate_block_post_execution, verify_receipts,
};

fn validate_dependency_proof<B: Block, C: EthChainSpec + EthereumHardforks>(
    block: &SealedBlock<B>,
    chain_spec: &C,
    proofs: &reth_storage_api::DependencyProofStore,
    require_proof: bool,
) -> Result<(), ConsensusError> {
    if !chain_spec.is_bogota_active_at_timestamp(block.header().timestamp()) {
        return Ok(());
    }
    let dependencies = alloy_consensus::TxEip8141::collect_signature_dependencies(
        block.body().transactions_iter(),
    )
    .map_err(ConsensusError::other)?;
    if block.header().block_deps_hash() !=
        Some(alloy_eips::eip8288::dependency_digest(&dependencies))
    {
        return Err(ConsensusError::DependencyCommitmentMismatch);
    }
    // Empty proofs can be verified from their commitment even when supplemental data was pruned.
    if dependencies.is_empty() {
        return if block.header().dependency_proof_hash() == Some(alloy_primitives::keccak256([])) {
            Ok(())
        } else {
            Err(ConsensusError::DependencyProofCommitmentMismatch)
        };
    }
    if !require_proof {
        return Ok(());
    }
    #[cfg(feature = "std")]
    {
        let proof = read_dependency_proof(
            proofs,
            block.hash(),
            block.header().dependency_proof_hash().ok_or(ConsensusError::MissingDependencyProof)?,
        )?;
        alloy_eips::eip8288::verify_proof(&dependencies, &proof).map_err(|err| {
            if err == alloy_eips::eip8288::ProofError::VerifierUnavailable {
                ConsensusError::DependencyProofProcessing(Arc::new(err))
            } else {
                ConsensusError::other(err)
            }
        })
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = proofs;
        Err(ConsensusError::DependencyProofUnsupported)
    }
}

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug, Clone)]
pub struct EthBeaconConsensus<ChainSpec> {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
    /// Supplemental dependency proofs shared with payload import.
    proofs: reth_storage_api::DependencyProofStore,
    /// Maximum allowed extra data size in bytes
    max_extra_data_size: usize,
    /// When true, skips the gas limit change validation between parent and child blocks.
    skip_gas_limit_ramp_check: bool,
    /// When true, skips the blob gas used check in header and post-execution validation.
    skip_blob_gas_used_check: bool,
    /// When true, skips the requests hash check in post-execution validation.
    skip_requests_hash_check: bool,
    /// When true, allows BAL hashes before Amsterdam activation.
    allow_bal_hashes: bool,
}

impl<ChainSpec: EthChainSpec + EthereumHardforks> EthBeaconConsensus<ChainSpec> {
    /// Create a new instance of [`EthBeaconConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            chain_spec,
            proofs: reth_storage_api::DependencyProofStore::unconfigured(),
            max_extra_data_size: MAXIMUM_EXTRA_DATA_SIZE,
            skip_gas_limit_ramp_check: false,
            skip_blob_gas_used_check: false,
            skip_requests_hash_check: false,
            allow_bal_hashes: false,
        }
    }

    /// Uses a durable store for separately transported dependency proofs.
    pub fn with_dependency_proofs(
        mut self,
        proofs: reth_storage_api::DependencyProofStore,
    ) -> Self {
        self.proofs = proofs;
        self
    }

    /// Returns the maximum allowed extra data size.
    pub const fn max_extra_data_size(&self) -> usize {
        self.max_extra_data_size
    }

    /// Sets the maximum allowed extra data size and returns the updated instance.
    pub const fn with_max_extra_data_size(mut self, size: usize) -> Self {
        self.max_extra_data_size = size;
        self
    }

    /// Disables the gas limit change validation between parent and child blocks.
    pub const fn with_skip_gas_limit_ramp_check(mut self, skip: bool) -> Self {
        self.skip_gas_limit_ramp_check = skip;
        self
    }

    /// Disables the blob gas used check in header and post-execution validation.
    pub const fn with_skip_blob_gas_used_check(mut self, skip: bool) -> Self {
        self.skip_blob_gas_used_check = skip;
        self
    }

    /// Disables the requests hash check in post-execution validation.
    pub const fn with_skip_requests_hash_check(mut self, skip: bool) -> Self {
        self.skip_requests_hash_check = skip;
        self
    }

    /// Allows BAL hashes before Amsterdam activation.
    pub const fn with_allow_bal_hashes(mut self, allow: bool) -> Self {
        self.allow_bal_hashes = allow;
        self
    }

    /// Returns the chain spec associated with this consensus engine.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec, N> FullConsensus<N> for EthBeaconConsensus<ChainSpec>
where
    ChainSpec: Send + EthChainSpec<Header = N::BlockHeader> + EthereumHardforks,
    N: NodePrimitives,
{
    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<N::Block>,
        result: &BlockExecutionResult<N::Receipt>,
        receipt_root_bloom: Option<ReceiptRootBloom>,
        block_access_list_hash: Option<B256>,
    ) -> Result<(), ConsensusError> {
        validation::validate_block_post_execution_with_bal_hashes(
            block,
            &self.chain_spec,
            result,
            receipt_root_bloom,
            block_access_list_hash,
            self.allow_bal_hashes,
            self.skip_blob_gas_used_check,
            self.skip_requests_hash_check,
        )
    }
}

impl<B, ChainSpec> Consensus<B> for EthBeaconConsensus<ChainSpec>
where
    B: Block,
    ChainSpec: EthChainSpec<Header = B::Header> + EthereumHardforks,
{
    #[cfg(feature = "std")]
    fn prepare_block_range_supplemental_data(
        &self,
        range: RangeInclusive<u64>,
    ) -> SupplementalDataFuture {
        let proofs = self.proofs.clone();
        Box::pin(async move {
            // A pre-activation range needs no supplemental downloads.
            let end = *range.end();
            let store = proofs.clone();
            let last = tokio::task::spawn_blocking(move || store.requests(end..=end))
                .await
                .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))?
                .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))?;
            if last.is_empty() {
                return Ok(())
            }
            let mut start = *range.start();
            while start <= end {
                let batch_end = start.saturating_add(127).min(end);
                let store = proofs.clone();
                let requests =
                    tokio::task::spawn_blocking(move || store.requests(start..=batch_end))
                        .await
                        .map_err(|error| {
                            ConsensusError::DependencyProofProcessing(Arc::new(error))
                        })?
                        .map_err(|error| {
                            ConsensusError::DependencyProofProcessing(Arc::new(error))
                        })?;
                for request in requests {
                    prepare_dependency_proof(proofs.clone(), request).await?;
                }
                if batch_end == end {
                    break
                }
                start = batch_end + 1;
            }
            Ok(())
        })
    }

    #[cfg(feature = "std")]
    fn prepare_block_supplemental_data(
        &self,
        header: SealedHeader<B::Header>,
    ) -> SupplementalDataFuture {
        let proofs = self.proofs.clone();
        Box::pin(async move {
            if let Some(commitment) = header.dependency_proof_hash() {
                prepare_dependency_proof(
                    proofs,
                    DependencyProofRequest { block_hash: header.hash(), commitment },
                )
                .await?;
            }
            Ok(())
        })
    }

    fn validate_block_supplemental_data(
        &self,
        block: &SealedBlock<B>,
    ) -> Result<(), ConsensusError> {
        validate_dependency_proof(block, self.chain_spec.as_ref(), &self.proofs, true)
    }

    fn validate_body_against_header(
        &self,
        body: &B::Body,
        header: &SealedHeader<B::Header>,
    ) -> Result<(), ConsensusError> {
        validate_body_against_header(body, header.header())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock<B>) -> Result<(), ConsensusError> {
        validate_block_pre_execution(block, &self.chain_spec)?;
        validate_dependency_proof(block, self.chain_spec.as_ref(), &self.proofs, false)
    }

    fn validate_block_pre_execution_with_tx_root(
        &self,
        block: &SealedBlock<B>,
        transaction_root: Option<TransactionRoot>,
    ) -> Result<(), ConsensusError> {
        validate_block_pre_execution_with_tx_root(block, &self.chain_spec, transaction_root)?;
        validate_dependency_proof(block, self.chain_spec.as_ref(), &self.proofs, false)
    }
}

impl<H, ChainSpec> HeaderValidator<H> for EthBeaconConsensus<ChainSpec>
where
    H: BlockHeader,
    ChainSpec: EthChainSpec<Header = H> + EthereumHardforks,
{
    fn validate_header(&self, header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        let header = header.header();
        let dependencies_active = self.chain_spec.is_bogota_active_at_timestamp(header.timestamp());
        let proof_hash = header.dependency_proof_hash();
        let deps_hash = header.block_deps_hash();
        if !dependencies_active && (proof_hash.is_some() || deps_hash.is_some()) {
            return Err(ConsensusError::UnexpectedDependencyProof);
        }
        if dependencies_active && (proof_hash.is_none() || deps_hash.is_none()) {
            return Err(ConsensusError::MissingDependencyProof);
        }

        let is_post_merge = self.chain_spec.is_paris_active_at_block(header.number());

        if is_post_merge {
            if !header.difficulty().is_zero() {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero);
            }

            if !header.nonce().is_some_and(|nonce| nonce.is_zero()) {
                return Err(ConsensusError::TheMergeNonceIsNotZero);
            }

            if header.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty);
            }
        } else {
            #[cfg(feature = "std")]
            {
                let present_timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                if header.timestamp() >
                    present_timestamp + alloy_eips::merge::ALLOWED_FUTURE_BLOCK_TIME_SECONDS
                {
                    return Err(ConsensusError::TimestampIsInFuture {
                        timestamp: header.timestamp(),
                        present_timestamp,
                    });
                }
            }
        }

        validate_header_extra_data(header, self.max_extra_data_size)?;
        validate_header_gas(header)?;
        validate_header_base_fee(header, &self.chain_spec)?;

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) &&
            header.withdrawals_root().is_none()
        {
            return Err(ConsensusError::WithdrawalsRootMissing)
        } else if !self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) &&
            header.withdrawals_root().is_some()
        {
            return Err(ConsensusError::WithdrawalsRootUnexpected)
        }

        // Ensures that EIP-4844 fields are valid once cancun is active.
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp()) {
            if !self.skip_blob_gas_used_check {
                validate_4844_header_standalone(
                    header,
                    self.chain_spec
                        .blob_params_at_timestamp(header.timestamp())
                        .unwrap_or_else(BlobParams::cancun),
                )?;
            }
        } else if header.blob_gas_used().is_some() {
            return Err(ConsensusError::BlobGasUsedUnexpected)
        } else if header.excess_blob_gas().is_some() {
            return Err(ConsensusError::ExcessBlobGasUnexpected)
        } else if header.parent_beacon_block_root().is_some() {
            return Err(ConsensusError::ParentBeaconBlockRootUnexpected)
        }

        if self.chain_spec.is_prague_active_at_timestamp(header.timestamp()) {
            if header.requests_hash().is_none() {
                return Err(ConsensusError::RequestsHashMissing)
            }
        } else if header.requests_hash().is_some() {
            return Err(ConsensusError::RequestsHashUnexpected)
        }

        if self.chain_spec.is_amsterdam_active_at_timestamp(header.timestamp()) {
            if header.block_access_list_hash().is_none() {
                return Err(ConsensusError::BlockAccessListHashMissing)
            }
            if header.slot_number().is_none() {
                return Err(ConsensusError::SlotNumberMissing)
            }
        } else {
            if header.block_access_list_hash().is_some() && !self.allow_bal_hashes {
                return Err(ConsensusError::BlockAccessListHashUnexpected)
            }
            if header.slot_number().is_some() {
                return Err(ConsensusError::SlotNumberUnexpected)
            }
        }

        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<H>,
        parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header.header(), parent)?;

        validate_against_parent_timestamp(header.header(), parent.header())?;

        if !self.skip_gas_limit_ramp_check {
            validate_against_parent_gas_limit(header, parent, &self.chain_spec)?;
        }

        validate_against_parent_eip1559_base_fee(
            header.header(),
            parent.header(),
            &self.chain_spec,
        )?;

        // ensure that the blob gas fields for this block
        if let Some(blob_params) = self.chain_spec.blob_params_at_timestamp(header.timestamp()) {
            validate_against_parent_4844(header.header(), parent.header(), blob_params)?;
        }

        Ok(())
    }
}

#[cfg(feature = "std")]
async fn prepare_dependency_proof(
    proofs: DependencyProofStore,
    request: DependencyProofRequest,
) -> Result<(), ConsensusError> {
    if request.commitment == alloy_primitives::keccak256([]) {
        return Ok(())
    }
    let store = proofs.clone();
    let available = tokio::task::spawn_blocking(move || {
        read_dependency_proof(&store, request.block_hash, request.commitment)
    })
    .await
    .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))?;
    if available.is_ok() {
        return Ok(())
    }
    let fetcher =
        proofs.fetcher().ok_or(ConsensusError::DependencyProofUnavailable(request.block_hash))?;
    let proof = fetcher
        .fetch(request)
        .await
        .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))?
        .ok_or(ConsensusError::DependencyProofUnavailable(request.block_hash))?;
    if proof.len() > alloy_eips::eip8288::MAX_DEPENDENCY_PROOF_BYTES ||
        alloy_primitives::keccak256(&proof) != request.commitment
    {
        return Err(ConsensusError::DependencyProofUnavailable(request.block_hash));
    }
    tokio::task::spawn_blocking(move || proofs.insert(request.block_hash, proof))
        .await
        .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))?
        .map_err(|error| ConsensusError::DependencyProofProcessing(Arc::new(error)))
}

// A bad local row is unavailable data, not evidence that the committed block is invalid.
#[cfg(any(feature = "std", test))]
fn read_dependency_proof(
    proofs: &reth_storage_api::DependencyProofStore,
    hash: B256,
    commitment: B256,
) -> Result<Bytes, ConsensusError> {
    let proof = proofs
        .get(hash)
        .map_err(|err| ConsensusError::DependencyProofProcessing(Arc::new(err)))?
        .ok_or(ConsensusError::DependencyProofUnavailable(hash))?;
    if proof.len() > alloy_eips::eip8288::MAX_DEPENDENCY_PROOF_BYTES ||
        alloy_primitives::keccak256(&proof) != commitment
    {
        return Err(ConsensusError::DependencyProofUnavailable(hash));
    }
    Ok(proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_eips::{eip4844::DATA_GAS_PER_BLOB, eip7685::EMPTY_REQUESTS_HASH};
    use alloy_primitives::B256;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_consensus_common::validation::validate_against_parent_gas_limit;
    use reth_ethereum_primitives::{Block as EthBlock, EthPrimitives, Receipt};
    use reth_primitives_traits::{
        constants::{GAS_LIMIT_BOUND_DIVISOR, MINIMUM_GAS_LIMIT},
        proofs, GotExpected,
    };

    #[cfg(feature = "std")]
    mod preparation {
        use super::*;
        use reth_storage_api::{
            DependencyProofFetcher, DependencyProofFuture, DependencyProofProvider,
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        };

        #[derive(Debug)]
        struct Fetcher {
            response: Option<Bytes>,
            calls: AtomicUsize,
        }

        impl DependencyProofFetcher for Fetcher {
            fn fetch(&self, _: DependencyProofRequest) -> DependencyProofFuture {
                self.calls.fetch_add(1, Ordering::Relaxed);
                let response = self.response.clone();
                Box::pin(async move { Ok(response) })
            }
        }

        #[derive(Debug)]
        struct Store {
            proof: Mutex<Option<Bytes>>,
            fetcher: Arc<Fetcher>,
        }

        impl DependencyProofProvider for Store {
            fn dependency_proof(
                &self,
                _: B256,
            ) -> reth_storage_api::errors::provider::ProviderResult<Option<Bytes>> {
                Ok(self.proof.lock().unwrap().clone())
            }
            fn insert_dependency_proof(
                &self,
                _: B256,
                proof: Bytes,
            ) -> reth_storage_api::errors::provider::ProviderResult<()> {
                *self.proof.lock().unwrap() = Some(proof);
                Ok(())
            }
            fn dependency_proof_fetcher(&self) -> Option<Arc<dyn DependencyProofFetcher>> {
                Some(self.fetcher.clone())
            }
        }

        #[tokio::test]
        async fn missing_and_corrupt_proofs_are_fetched_once_and_persisted() {
            let proof = Bytes::from_static(b"proof");
            let request = DependencyProofRequest {
                block_hash: B256::repeat_byte(1),
                commitment: alloy_primitives::keccak256(&proof),
            };
            for local in [None, Some(Bytes::from_static(b"corrupt"))] {
                let store = Arc::new(Store {
                    proof: Mutex::new(local),
                    fetcher: Arc::new(Fetcher {
                        response: Some(proof.clone()),
                        calls: AtomicUsize::new(0),
                    }),
                });
                let proofs = DependencyProofStore::new(store.clone());
                prepare_dependency_proof(proofs.clone(), request).await.unwrap();
                prepare_dependency_proof(proofs, request).await.unwrap();
                assert_eq!(*store.proof.lock().unwrap(), Some(proof.clone()));
                assert_eq!(store.fetcher.calls.load(Ordering::Relaxed), 1);
            }
        }

        #[tokio::test]
        async fn unavailable_or_mismatching_peer_data_is_not_persisted() {
            let request = DependencyProofRequest {
                block_hash: B256::repeat_byte(1),
                commitment: alloy_primitives::keccak256(b"proof"),
            };
            for response in [None, Some(Bytes::from_static(b"wrong"))] {
                let store = Arc::new(Store {
                    proof: Mutex::new(None),
                    fetcher: Arc::new(Fetcher { response, calls: AtomicUsize::new(0) }),
                });
                assert!(
                    matches!(prepare_dependency_proof(DependencyProofStore::new(store.clone()), request).await,
                    Err(ConsensusError::DependencyProofUnavailable(hash)) if hash == request.block_hash)
                );
                assert_eq!(*store.proof.lock().unwrap(), None);
            }
        }
    }

    #[derive(Debug)]
    struct StoredProof(Option<Bytes>);

    impl reth_storage_api::DependencyProofProvider for StoredProof {
        fn dependency_proof(
            &self,
            _hash: B256,
        ) -> reth_storage_api::errors::provider::ProviderResult<Option<Bytes>> {
            Ok(self.0.clone())
        }
        fn insert_dependency_proof(
            &self,
            _hash: B256,
            _proof: Bytes,
        ) -> reth_storage_api::errors::provider::ProviderResult<()> {
            Err(reth_storage_api::errors::provider::ProviderError::UnsupportedProvider)
        }
    }

    #[test]
    fn missing_or_corrupt_local_proofs_do_not_invalidate_blocks() {
        let hash = B256::repeat_byte(1);
        let proof = Bytes::from_static(&[1, 2, 3]);
        let commitment = alloy_primitives::keccak256(&proof);
        for data in [
            None,
            Some(Bytes::new()),
            Some(vec![0; alloy_eips::eip8288::MAX_DEPENDENCY_PROOF_BYTES + 1].into()),
        ] {
            let store = reth_storage_api::DependencyProofStore::new(StoredProof(data));
            let error = read_dependency_proof(&store, hash, commitment).unwrap_err();
            assert!(
                matches!(error, ConsensusError::DependencyProofUnavailable(found) if found == hash)
            );
            assert!(error.is_dependency_proof_unavailable());
        }
        let store = reth_storage_api::DependencyProofStore::new(StoredProof(Some(proof.clone())));
        assert_eq!(read_dependency_proof(&store, hash, commitment).unwrap(), proof);
    }

    fn header_with_gas_limit(gas_limit: u64) -> SealedHeader {
        let header = reth_primitives_traits::Header { gas_limit, ..Default::default() };
        SealedHeader::new(header, B256::ZERO)
    }

    #[test]
    fn dependency_commitments_enforce_activation_presence_and_contents() {
        let spec = ChainSpecBuilder::mainnet()
            .bogota_activated()
            .with_fork(
                reth_chainspec::EthereumHardfork::Bogota,
                reth_chainspec::ForkCondition::Timestamp(1),
            )
            .build();
        let consensus = EthBeaconConsensus::new(Arc::new(spec));
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(alloy_eips::eip7928::EMPTY_BLOCK_ACCESS_LIST_HASH);
        header.slot_number = Some(1);
        header.dependency_proof_hash = Some(alloy_primitives::keccak256([]));
        header.block_deps_hash = Some(alloy_eips::eip8288::dependency_digest(&[]));
        assert!(matches!(
            consensus.validate_header(&SealedHeader::seal_slow(header.clone())),
            Err(ConsensusError::UnexpectedDependencyProof)
        ));
        header.timestamp = 1;
        let block = SealedBlock::new_unhashed(EthBlock {
            header: header.clone(),
            body: Default::default(),
        });
        assert!(consensus.validate_block_supplemental_data(&block).is_ok());
        header.block_deps_hash = Some(B256::ZERO);
        let block = SealedBlock::new_unhashed(EthBlock {
            header: header.clone(),
            body: Default::default(),
        });
        assert!(matches!(
            consensus.validate_block_supplemental_data(&block),
            Err(ConsensusError::DependencyCommitmentMismatch)
        ));
        header.block_deps_hash = None;
        assert!(matches!(
            consensus.validate_header(&SealedHeader::seal_slow(header)),
            Err(ConsensusError::MissingDependencyProof)
        ));
    }

    fn valid_prague_header() -> reth_primitives_traits::Header {
        reth_primitives_traits::Header {
            base_fee_per_gas: Some(1337),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: Some(B256::ZERO),
            requests_hash: Some(EMPTY_REQUESTS_HASH),
            ..Default::default()
        }
    }

    fn prague_recovered_block_with_bal_hash(hash: B256) -> RecoveredBlock<EthBlock> {
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(hash);
        RecoveredBlock::new_unhashed(EthBlock { header, body: Default::default() }, Vec::new())
    }

    #[test]
    fn test_valid_gas_limit_increase() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(parent.gas_limit + 5);

        assert!(validate_against_parent_gas_limit(
            &child,
            &parent,
            &ChainSpec::<Header>::default()
        )
        .is_ok());
    }

    #[test]
    fn test_gas_limit_below_minimum() {
        let parent = header_with_gas_limit(MINIMUM_GAS_LIMIT);
        let child = header_with_gas_limit(MINIMUM_GAS_LIMIT - 1);

        assert!(matches!(
            validate_against_parent_gas_limit(&child, &parent, &ChainSpec::<Header>::default()).unwrap_err(),
            ConsensusError::GasLimitInvalidMinimum { child_gas_limit }
                if child_gas_limit == child.gas_limit
        ));
    }

    #[test]
    fn test_invalid_gas_limit_increase_exceeding_limit() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(
            parent.gas_limit + parent.gas_limit / GAS_LIMIT_BOUND_DIVISOR + 1,
        );

        assert!(matches!(
            validate_against_parent_gas_limit(&child, &parent, &ChainSpec::<Header>::default()).unwrap_err(),
            ConsensusError::GasLimitInvalidIncrease { parent_gas_limit, child_gas_limit }
                if parent_gas_limit == parent.gas_limit && child_gas_limit == child.gas_limit
        ));
    }

    #[test]
    fn test_valid_gas_limit_decrease_within_limit() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(parent.gas_limit - 5);

        assert!(validate_against_parent_gas_limit(
            &child,
            &parent,
            &ChainSpec::<Header>::default()
        )
        .is_ok());
    }

    #[test]
    fn test_invalid_gas_limit_decrease_exceeding_limit() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(
            parent.gas_limit - parent.gas_limit / GAS_LIMIT_BOUND_DIVISOR - 1,
        );

        assert!(matches!(
            validate_against_parent_gas_limit(&child, &parent, &ChainSpec::<Header>::default()).unwrap_err(),
            ConsensusError::GasLimitInvalidDecrease { parent_gas_limit, child_gas_limit }
                if parent_gas_limit == parent.gas_limit && child_gas_limit == child.gas_limit
        ));
    }

    #[test]
    fn shanghai_block_zero_withdrawals() {
        // ensures that if shanghai is activated, and we include a block with a withdrawals root,
        // that the header is valid
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());

        let header = reth_primitives_traits::Header {
            base_fee_per_gas: Some(1337),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            ..Default::default()
        };

        assert!(EthBeaconConsensus::new(chain_spec)
            .validate_header(&SealedHeader::seal_slow(header,))
            .is_ok());
    }

    #[test]
    fn prague_header_rejects_block_access_list_hash_before_amsterdam() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);

        assert!(matches!(
            EthBeaconConsensus::new(chain_spec)
                .validate_header(&SealedHeader::seal_slow(header,))
                .unwrap_err(),
            ConsensusError::BlockAccessListHashUnexpected
        ));
    }

    #[test]
    fn prague_header_allows_block_access_list_hash_before_amsterdam() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);

        assert!(EthBeaconConsensus::new(chain_spec)
            .with_allow_bal_hashes(true)
            .validate_header(&SealedHeader::seal_slow(header,))
            .is_ok());
    }

    #[test]
    fn prague_header_rejects_slot_number_before_amsterdam() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.slot_number = Some(0);

        assert!(matches!(
            EthBeaconConsensus::new(chain_spec)
                .validate_header(&SealedHeader::seal_slow(header,))
                .unwrap_err(),
            ConsensusError::SlotNumberUnexpected
        ));
    }

    #[test]
    fn prague_header_rejects_slot_number_with_allowed_bal_hashes_before_amsterdam() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);
        header.slot_number = Some(0);

        assert!(matches!(
            EthBeaconConsensus::new(chain_spec)
                .with_allow_bal_hashes(true)
                .validate_header(&SealedHeader::seal_slow(header,))
                .unwrap_err(),
            ConsensusError::SlotNumberUnexpected
        ));
    }

    #[test]
    fn prague_post_execution_allows_block_access_list_hash_before_amsterdam() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let expected_hash = B256::repeat_byte(0x42);
        let block = prague_recovered_block_with_bal_hash(expected_hash);
        let result = BlockExecutionResult::<Receipt>::default();
        let consensus = EthBeaconConsensus::new(chain_spec).with_allow_bal_hashes(true);

        assert!(FullConsensus::<EthPrimitives>::validate_block_post_execution(
            &consensus,
            &block,
            &result,
            None,
            Some(expected_hash),
        )
        .is_ok());

        assert!(FullConsensus::<EthPrimitives>::validate_block_post_execution(
            &consensus, &block, &result, None, None,
        )
        .is_ok());

        assert!(matches!(
            FullConsensus::<EthPrimitives>::validate_block_post_execution(
                &consensus,
                &block,
                &result,
                None,
                Some(B256::repeat_byte(0x24)),
            )
            .unwrap_err(),
            ConsensusError::BlockAccessListHashMismatch(_)
        ));
    }

    #[test]
    fn amsterdam_header_requires_block_access_list_hash() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build());
        let mut header = valid_prague_header();
        header.slot_number = Some(0);

        assert!(matches!(
            EthBeaconConsensus::new(chain_spec)
                .validate_header(&SealedHeader::seal_slow(header,))
                .unwrap_err(),
            ConsensusError::BlockAccessListHashMissing
        ));
    }

    #[test]
    fn amsterdam_header_requires_slot_number() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);

        assert!(matches!(
            EthBeaconConsensus::new(chain_spec)
                .validate_header(&SealedHeader::seal_slow(header,))
                .unwrap_err(),
            ConsensusError::SlotNumberMissing
        ));
    }

    #[test]
    fn amsterdam_header_accepts_block_access_list_hash_and_slot_number() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);
        header.slot_number = Some(0);

        assert!(EthBeaconConsensus::new(chain_spec)
            .validate_header(&SealedHeader::seal_slow(header,))
            .is_ok());
    }

    #[test]
    fn amsterdam_post_execution_requires_computed_block_access_list_hash() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build());
        let block = prague_recovered_block_with_bal_hash(B256::ZERO);
        let result = BlockExecutionResult::<Receipt>::default();
        let consensus = EthBeaconConsensus::new(chain_spec);

        assert!(matches!(
            FullConsensus::<EthPrimitives>::validate_block_post_execution(
                &consensus, &block, &result, None, None,
            )
            .unwrap_err(),
            ConsensusError::BlockAccessListHashMissing
        ));
    }

    #[test]
    fn skip_requests_hash_check_still_validates_block_access_list_hash() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build());
        let mut header = valid_prague_header();
        header.block_access_list_hash = Some(B256::ZERO);
        header.requests_hash = Some(B256::with_last_byte(1));
        let block =
            RecoveredBlock::new_unhashed(EthBlock { header, body: Default::default() }, Vec::new());
        let result = BlockExecutionResult::<Receipt>::default();
        let computed_bal_hash = B256::with_last_byte(2);

        let consensus = EthBeaconConsensus::new(chain_spec).with_skip_requests_hash_check(true);
        assert!(matches!(
            FullConsensus::<EthPrimitives>::validate_block_post_execution(
                &consensus,
                &block,
                &result,
                None,
                Some(computed_bal_hash),
            )
            .unwrap_err(),
            ConsensusError::BlockAccessListHashMismatch(_)
        ));

        // A requests hash mismatch alone is skipped.
        assert!(FullConsensus::<EthPrimitives>::validate_block_post_execution(
            &consensus,
            &block,
            &result,
            None,
            Some(B256::ZERO),
        )
        .is_ok());
    }

    #[test]
    fn prague_post_execution_rejects_blob_gas_used_diff_for_empty_block() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.blob_gas_used = Some(DATA_GAS_PER_BLOB);
        let block =
            RecoveredBlock::new_unhashed(EthBlock { header, body: Default::default() }, Vec::new());
        let result = BlockExecutionResult::<Receipt>::default();

        let consensus = EthBeaconConsensus::new(chain_spec.clone());
        assert!(matches!(
            FullConsensus::<EthPrimitives>::validate_block_post_execution(
                &consensus, &block, &result, None, None,
            )
            .unwrap_err(),
            ConsensusError::BlobGasUsedDiff(GotExpected { got: 0, expected: DATA_GAS_PER_BLOB })
        ));

        let consensus = EthBeaconConsensus::new(chain_spec).with_skip_blob_gas_used_check(true);
        assert!(FullConsensus::<EthPrimitives>::validate_block_post_execution(
            &consensus, &block, &result, None, None,
        )
        .is_ok());
    }

    #[test]
    fn prague_post_execution_requires_blob_gas_used() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let mut header = valid_prague_header();
        header.blob_gas_used = None;
        let block =
            RecoveredBlock::new_unhashed(EthBlock { header, body: Default::default() }, Vec::new());
        let result = BlockExecutionResult::<Receipt>::default();
        let consensus = EthBeaconConsensus::new(chain_spec);

        assert!(matches!(
            FullConsensus::<EthPrimitives>::validate_block_post_execution(
                &consensus, &block, &result, None, None,
            )
            .unwrap_err(),
            ConsensusError::BlobGasUsedMissing
        ));
    }
}
