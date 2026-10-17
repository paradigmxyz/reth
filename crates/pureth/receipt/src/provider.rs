use crate::eip6466::{Eip6466ReceiptSnapshot, Eip6466SnapshotError};
use alloy_consensus::{BlockHeader as _, Header, TxLegacy, TxType};
use alloy_primitives::{Address, Bytes, Log as ExecutionLog, Signature, TxKind, B256};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_ethereum_primitives::{
    Block, BlockBody, EthPrimitives, Receipt as StoredReceipt, Transaction as EthereumTransaction,
    TransactionSigned,
};
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    BlockHashReader, BlockReader, ProviderError, ReceiptProvider, TransactionVariant,
};
use std::fmt;

pub const SINGLETON_BLOCK_HASH: B256 =
    alloy_primitives::b256!("0101010101010101010101010101010101010101010101010101010101010101");
pub const MULTIPLE_LOGS_BLOCK_HASH: B256 =
    alloy_primitives::b256!("0202020202020202020202020202020202020202020202020202020202020202");
pub const PROGRESSIVE_RECEIPTS_BLOCK_HASH: B256 =
    alloy_primitives::b256!("0303030303030303030303030303030303030303030303030303030303030303");

#[derive(Debug)]
pub struct DeterministicProvider {
    snapshots: [ProviderSnapshot; 3],
}

impl DeterministicProvider {
    pub fn new() -> Result<Self, ProviderBuildError> {
        Ok(Self {
            snapshots: [
                singleton_snapshot()?,
                multiple_logs_snapshot()?,
                progressive_receipts_snapshot()?,
            ],
        })
    }

    pub fn lookup(
        &self,
        block_hash: B256,
        object: ObjectKind,
    ) -> Result<&ProviderSnapshot, LookupError> {
        validate_lookup(object)?;

        self.snapshots
            .iter()
            .find(|snapshot| snapshot.block_hash == block_hash)
            .ok_or(LookupError::UnknownBlock)
    }
}

#[derive(Debug)]
pub struct RethRootProvider<N: ProviderNodeTypes<Primitives = EthPrimitives>> {
    provider: BlockchainProvider<N>,
}

impl<N> RethRootProvider<N>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
{
    pub const fn new(provider: BlockchainProvider<N>) -> Self {
        Self { provider }
    }

    pub fn lookup(
        &self,
        block_hash: B256,
        object: ObjectKind,
    ) -> Result<ProviderSnapshot, RethRootProviderError>
    where
        N::ChainSpec: EthereumHardforks,
    {
        validate_lookup(object).map_err(RethRootProviderError::Lookup)?;

        ProviderSnapshot::from_reth_historical(&self.provider, block_hash)
            .map_err(map_historical_lookup_error)
    }
}

#[derive(Debug)]
pub struct ProviderSnapshot {
    block_hash: B256,
    block_status: CanonicalityStatus,
    object: ObjectKind,
    root_context: RootContext,
    receipt_snapshot: Eip6466ReceiptSnapshot,
}

impl ProviderSnapshot {
    pub fn from_reth_historical<N>(
        provider: &BlockchainProvider<N>,
        block_hash: B256,
    ) -> Result<Self, HistoricalAcquisitionError>
    where
        N: ProviderNodeTypes<Primitives = EthPrimitives>,
        N::ChainSpec: EthereumHardforks,
    {
        let (block, block_status, receipts) = {
            let view = provider
                .consistent_provider()
                .map_err(HistoricalAcquisitionError::ConsistentView)?;
            let block = view
                .recovered_block(block_hash.into(), TransactionVariant::WithHash)
                .map_err(HistoricalAcquisitionError::BlockRead)?
                .ok_or(HistoricalAcquisitionError::BlockUnavailable)?;
            let canonical_hash = view
                .block_hash(block.number())
                .map_err(HistoricalAcquisitionError::CanonicalHashRead)?;
            let block_status = derive_canonicality(block_hash, canonical_hash);
            let receipts = view
                .receipts_by_block(block_hash.into())
                .map_err(HistoricalAcquisitionError::ReceiptsRead)?
                .ok_or(HistoricalAcquisitionError::ReceiptsUnavailable)?;
            (block, block_status, receipts)
        };

        let eip658_active = provider.chain_spec().is_byzantium_active_at_block(block.number());
        let receipt_snapshot =
            Eip6466ReceiptSnapshot::from_block(&block, &receipts, eip658_active, None)
                .map_err(HistoricalAcquisitionError::Snapshot)?;

        Ok(Self {
            block_hash,
            block_status,
            object: ObjectKind::Receipts,
            root_context: RootContext::RethExperimentalUnanchored,
            receipt_snapshot,
        })
    }

    pub const fn block_hash(&self) -> B256 {
        self.block_hash
    }

    pub const fn block_status(&self) -> CanonicalityStatus {
        self.block_status
    }

    pub const fn object(&self) -> ObjectKind {
        self.object
    }

    pub const fn root_context(&self) -> RootContext {
        self.root_context
    }

    pub const fn receipt_snapshot(&self) -> &Eip6466ReceiptSnapshot {
        &self.receipt_snapshot
    }

    pub const fn root(&self) -> B256 {
        self.receipt_snapshot.root()
    }
}

fn derive_canonicality(block_hash: B256, canonical_hash: Option<B256>) -> CanonicalityStatus {
    match canonical_hash {
        Some(canonical_hash) if canonical_hash == block_hash => CanonicalityStatus::Canonical,
        Some(_) => CanonicalityStatus::NonCanonical,
        None => CanonicalityStatus::Unknown,
    }
}

fn validate_lookup(object: ObjectKind) -> Result<(), LookupError> {
    if object != ObjectKind::Receipts {
        return Err(LookupError::UnsupportedObject);
    }
    Ok(())
}

fn map_historical_lookup_error(error: HistoricalAcquisitionError) -> RethRootProviderError {
    match error {
        HistoricalAcquisitionError::BlockUnavailable => {
            RethRootProviderError::Lookup(LookupError::UnknownBlock)
        }
        error => RethRootProviderError::Acquisition(error),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Receipts,
    Withdrawals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalityStatus {
    Canonical,
    NonCanonical,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootContext {
    DeterministicTestData,
    RethExperimentalUnanchored,
}

#[derive(Debug)]
pub enum ProviderBuildError {
    RecoveredBlock,
    Snapshot(Eip6466SnapshotError),
}

impl fmt::Display for ProviderBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RecoveredBlock => formatter.write_str("deterministic block recovery failed"),
            Self::Snapshot(error) => {
                write!(formatter, "receipt snapshot construction failed: {error}")
            }
        }
    }
}

impl std::error::Error for ProviderBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RecoveredBlock => None,
            Self::Snapshot(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub enum HistoricalAcquisitionError {
    ConsistentView(ProviderError),
    BlockRead(ProviderError),
    CanonicalHashRead(ProviderError),
    ReceiptsRead(ProviderError),
    BlockUnavailable,
    ReceiptsUnavailable,
    Snapshot(Eip6466SnapshotError),
}

impl HistoricalAcquisitionError {
    pub const fn is_unavailable(&self) -> bool {
        match self {
            Self::BlockUnavailable | Self::ReceiptsUnavailable => true,
            Self::BlockRead(error) => matches!(
                error,
                ProviderError::BlockExpired { .. } |
                    ProviderError::BlockHashNotFound(_) |
                    ProviderError::UnknownBlockHash(_) |
                    ProviderError::HeaderNotFound(_)
            ),
            Self::ReceiptsRead(error) => matches!(
                error,
                ProviderError::BlockExpired { .. } | ProviderError::ReceiptNotFound(_)
            ),
            Self::Snapshot(error) => matches!(error, Eip6466SnapshotError::MissingData(_)),
            Self::ConsistentView(_) | Self::CanonicalHashRead(_) => false,
        }
    }
}

impl fmt::Display for HistoricalAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConsistentView(error) => {
                write!(formatter, "consistent provider view failed: {error}")
            }
            Self::BlockRead(error) => write!(formatter, "historical block read failed: {error}"),
            Self::CanonicalHashRead(error) => {
                write!(formatter, "canonical hash read failed: {error}")
            }
            Self::ReceiptsRead(error) => {
                write!(formatter, "historical receipt read failed: {error}")
            }
            Self::BlockUnavailable => formatter.write_str("requested block is unavailable"),
            Self::ReceiptsUnavailable => formatter.write_str("requested receipts are unavailable"),
            Self::Snapshot(error) => {
                write!(formatter, "historical receipt snapshot construction failed: {error}")
            }
        }
    }
}

impl std::error::Error for HistoricalAcquisitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConsistentView(error) |
            Self::BlockRead(error) |
            Self::CanonicalHashRead(error) |
            Self::ReceiptsRead(error) => Some(error),
            Self::Snapshot(error) => Some(error),
            Self::BlockUnavailable | Self::ReceiptsUnavailable => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupError {
    UnknownBlock,
    UnsupportedObject,
}

impl fmt::Display for LookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownBlock => "requested block is unknown",
            Self::UnsupportedObject => "requested object is unsupported",
        })
    }
}

impl std::error::Error for LookupError {}

#[derive(Debug)]
pub enum RethRootProviderError {
    Lookup(LookupError),
    Acquisition(HistoricalAcquisitionError),
}

impl fmt::Display for RethRootProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lookup(error) => write!(formatter, "receipt lookup failed: {error}"),
            Self::Acquisition(error) => write!(formatter, "receipt acquisition failed: {error}"),
        }
    }
}

impl std::error::Error for RethRootProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lookup(error) => Some(error),
            Self::Acquisition(error) => Some(error),
        }
    }
}

fn singleton_snapshot() -> Result<ProviderSnapshot, ProviderBuildError> {
    build_snapshot(
        SINGLETON_BLOCK_HASH,
        vec![transaction(0)],
        vec![receipt(
            21_000,
            vec![log(
                Address::repeat_byte(0x11),
                vec![B256::repeat_byte(0x22)],
                &[0x01, 0x02, 0x03],
            )],
        )],
    )
}

fn multiple_logs_snapshot() -> Result<ProviderSnapshot, ProviderBuildError> {
    build_snapshot(
        MULTIPLE_LOGS_BLOCK_HASH,
        vec![transaction(0)],
        vec![receipt(
            21_000,
            vec![
                log(Address::repeat_byte(0x11), vec![B256::repeat_byte(0x22)], &[0x01, 0x02, 0x03]),
                log(Address::repeat_byte(0x33), vec![B256::repeat_byte(0x44)], &[0x04, 0x05]),
            ],
        )],
    )
}

fn progressive_receipts_snapshot() -> Result<ProviderSnapshot, ProviderBuildError> {
    let transactions = (0_u8..6).map(|index| transaction(u64::from(index))).collect();
    let receipts = (0_u8..6)
        .map(|index| {
            receipt(
                21_000 * (u64::from(index) + 1),
                vec![log(
                    Address::repeat_byte(0x11 + index),
                    vec![B256::repeat_byte(0x22)],
                    &[0x01, 0x02, 0x03],
                )],
            )
        })
        .collect();

    build_snapshot(PROGRESSIVE_RECEIPTS_BLOCK_HASH, transactions, receipts)
}

fn build_snapshot(
    block_hash: B256,
    transactions: Vec<TransactionSigned>,
    receipts: Vec<StoredReceipt>,
) -> Result<ProviderSnapshot, ProviderBuildError> {
    let gas_used = receipts.last().map_or(0, |receipt| receipt.cumulative_gas_used);
    let senders = vec![Address::repeat_byte(0x66); transactions.len()];
    let block = RecoveredBlock::try_new_unhashed(
        Block {
            header: Header { gas_used, gas_limit: gas_used, ..Default::default() },
            body: BlockBody { transactions, ..Default::default() },
        },
        senders,
    )
    .map_err(|_| ProviderBuildError::RecoveredBlock)?;
    let receipt_snapshot = Eip6466ReceiptSnapshot::from_block(&block, &receipts, true, None)
        .map_err(ProviderBuildError::Snapshot)?;

    Ok(ProviderSnapshot {
        block_hash,
        block_status: CanonicalityStatus::Unknown,
        object: ObjectKind::Receipts,
        root_context: RootContext::DeterministicTestData,
        receipt_snapshot,
    })
}

fn transaction(nonce: u64) -> TransactionSigned {
    TransactionSigned::new_unhashed(
        EthereumTransaction::Legacy(TxLegacy {
            nonce,
            to: TxKind::Call(Address::ZERO),
            ..Default::default()
        }),
        Signature::test_signature(),
    )
}

const fn receipt(cumulative_gas_used: u64, logs: Vec<ExecutionLog>) -> StoredReceipt {
    StoredReceipt { tx_type: TxType::Legacy, success: true, cumulative_gas_used, logs }
}

fn log(address: Address, topics: Vec<B256>, data: &[u8]) -> ExecutionLog {
    ExecutionLog::new_unchecked(address, topics, Bytes::copy_from_slice(data))
}

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_provider::{
        test_utils::{create_test_provider_factory_with_chain_spec, MockNodeTypesWithDB},
        BlockWriter, DBProvider, DatabaseProviderFactory, ExecutionOutcome, OriginalValuesKnown,
        StateWriteConfig, StateWriter,
    };
    use std::sync::Arc;

    pub fn historical_provider(
        receipts: Option<Vec<StoredReceipt>>,
    ) -> (BlockchainProvider<MockNodeTypesWithDB>, B256) {
        let block = RecoveredBlock::try_new_unhashed(
            Block {
                header: Header { gas_used: 21_000, gas_limit: 30_000, ..Default::default() },
                body: BlockBody { transactions: vec![transaction(0)], ..Default::default() },
            },
            vec![Address::repeat_byte(0x66)],
        )
        .unwrap();
        historical_provider_for_block(
            block,
            receipts,
            Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build()),
        )
    }

    pub fn historical_provider_for_block(
        block: RecoveredBlock<Block>,
        receipts: Option<Vec<StoredReceipt>>,
        chain_spec: Arc<ChainSpec>,
    ) -> (BlockchainProvider<MockNodeTypesWithDB>, B256) {
        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let provider = factory.database_provider_rw().unwrap();
        let block_hash = block.hash();
        provider.insert_block(&block).unwrap();
        if let Some(receipts) = receipts {
            provider
                .write_state(
                    &ExecutionOutcome {
                        first_block: block.number(),
                        receipts: vec![receipts],
                        ..Default::default()
                    },
                    OriginalValuesKnown::No,
                    StateWriteConfig::default(),
                )
                .unwrap();
        }
        provider.commit().unwrap();
        (BlockchainProvider::new(factory).unwrap(), block_hash)
    }

    pub fn singleton_receipts() -> Vec<StoredReceipt> {
        vec![receipt(
            21_000,
            vec![log(Address::repeat_byte(0x11), vec![B256::repeat_byte(0x22)], &[1, 2, 3])],
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReceiptConstructionError, Receipts, TreeConstructionError};
    use std::error::Error;

    #[test]
    fn historical_provider_returns_a_coherent_eip_snapshot() {
        let (provider, block_hash) =
            test_utils::historical_provider(Some(test_utils::singleton_receipts()));
        let result =
            RethRootProvider::new(provider).lookup(block_hash, ObjectKind::Receipts).unwrap();
        let snapshot = result.receipt_snapshot();
        assert_eq!(result.block_hash(), block_hash);
        assert_eq!(result.block_status(), CanonicalityStatus::Canonical);
        assert_eq!(result.object(), ObjectKind::Receipts);
        assert_eq!(result.root_context(), RootContext::RethExperimentalUnanchored);
        assert_eq!(snapshot.receipts().len(), 1);
        assert_eq!(
            snapshot.receipts().get(0).unwrap().logs()[0].address(),
            Address::repeat_byte(0x11)
        );
        assert_eq!(result.root(), snapshot.root());
        assert_eq!(result.root(), snapshot.tree().root());
        assert_eq!(Receipts::from_ssz_bytes(snapshot.serialized()).unwrap(), *snapshot.receipts());
    }

    #[test]
    fn historical_provider_preserves_missing_data_and_conversion_errors() {
        let (provider, block_hash) = test_utils::historical_provider(None);
        assert!(matches!(
            RethRootProvider::new(provider).lookup(block_hash, ObjectKind::Receipts),
            Err(RethRootProviderError::Acquisition(
                HistoricalAcquisitionError::ReceiptsUnavailable
            ))
        ));

        let receipts = vec![StoredReceipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 21_000,
            logs: Vec::new(),
        }];
        let (provider, block_hash) = test_utils::historical_provider(Some(receipts));
        assert!(matches!(
            RethRootProvider::new(provider).lookup(block_hash, ObjectKind::Receipts),
            Err(RethRootProviderError::Acquisition(HistoricalAcquisitionError::Snapshot(
                Eip6466SnapshotError::Conversion(
                    ReceiptConstructionError::TransactionTypeMismatch { .. }
                )
            )))
        ));
    }

    #[test]
    fn deterministic_cases_use_coherent_distinct_stored_snapshots() {
        let provider = DeterministicProvider::new().unwrap();
        let cases = [
            (SINGLETON_BLOCK_HASH, 0, 0, 1, Address::repeat_byte(0x11)),
            (MULTIPLE_LOGS_BLOCK_HASH, 0, 1, 1, Address::repeat_byte(0x33)),
            (PROGRESSIVE_RECEIPTS_BLOCK_HASH, 5, 0, 6, Address::repeat_byte(0x16)),
        ];
        let mut roots = Vec::new();
        for (block_hash, receipt_index, log_index, count, address) in cases {
            let first = provider.lookup(block_hash, ObjectKind::Receipts).unwrap();
            let second = provider.lookup(block_hash, ObjectKind::Receipts).unwrap();
            let snapshot = first.receipt_snapshot();
            assert!(std::ptr::eq(first, second));
            assert!(std::ptr::eq(snapshot, second.receipt_snapshot()));
            assert_eq!(first.block_hash(), block_hash);
            assert_eq!(first.block_status(), CanonicalityStatus::Unknown);
            assert_eq!(first.object(), ObjectKind::Receipts);
            assert_eq!(first.root_context(), RootContext::DeterministicTestData);
            assert_eq!(snapshot.receipts().len(), count);
            assert_eq!(
                snapshot.receipts().get(receipt_index).unwrap().logs()[log_index].address(),
                address
            );
            assert_eq!(first.root(), snapshot.root());
            assert_eq!(first.root(), snapshot.tree().root());
            assert_eq!(
                Receipts::from_ssz_bytes(snapshot.serialized()).unwrap(),
                *snapshot.receipts()
            );
            assert!(!roots.contains(&first.root()));
            roots.push(first.root());
        }
    }

    #[test]
    fn lookup_validates_object_before_block() {
        let provider = DeterministicProvider::new().unwrap();
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Withdrawals),
            Err(LookupError::UnsupportedObject)
        ));
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Receipts),
            Err(LookupError::UnknownBlock)
        ));
        assert!(matches!(
            provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Withdrawals),
            Err(LookupError::UnsupportedObject)
        ));

        let (provider, _) = test_utils::historical_provider(Some(test_utils::singleton_receipts()));
        let provider = RethRootProvider::new(provider);
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Withdrawals),
            Err(RethRootProviderError::Lookup(LookupError::UnsupportedObject))
        ));
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Receipts),
            Err(RethRootProviderError::Lookup(LookupError::UnknownBlock))
        ));
    }

    #[test]
    fn acquisition_errors_preserve_stages_and_unavailability() {
        let read = HistoricalAcquisitionError::BlockRead(ProviderError::BlockExpired {
            requested: 1,
            earliest_available: 2,
        });
        assert!(read.is_unavailable());
        let error = map_historical_lookup_error(read);
        assert!(error.to_string().contains("historical block read failed"));
        let source = error.source().unwrap();
        assert!(source.downcast_ref::<HistoricalAcquisitionError>().is_some());
        assert!(source.source().unwrap().downcast_ref::<ProviderError>().is_some());
        assert!(HistoricalAcquisitionError::BlockUnavailable.is_unavailable());
        assert!(HistoricalAcquisitionError::ReceiptsUnavailable.is_unavailable());
        assert!(!HistoricalAcquisitionError::ConsistentView(ProviderError::BlockExpired {
            requested: 1,
            earliest_available: 2
        },)
        .is_unavailable());
        assert!(!HistoricalAcquisitionError::BlockRead(ProviderError::InvalidStorageOutput)
            .is_unavailable());

        let construction =
            ReceiptConstructionError::InputCountMismatch { transactions: 1, receipts: 2 };
        let error =
            HistoricalAcquisitionError::Snapshot(Eip6466SnapshotError::Conversion(construction));
        assert!(!error.is_unavailable());
        assert!(error.source().unwrap().downcast_ref::<Eip6466SnapshotError>().is_some());
        assert!(error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<ReceiptConstructionError>()
            .is_some());

        let error = ProviderBuildError::Snapshot(Eip6466SnapshotError::Tree(
            TreeConstructionError::InvalidWidth { width: 0 },
        ));
        assert!(error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<TreeConstructionError>()
            .is_some());
        assert!(ProviderBuildError::RecoveredBlock.source().is_none());
    }

    #[test]
    fn canonicality_uses_the_same_view_hash() {
        let hash = B256::repeat_byte(1);
        assert_eq!(derive_canonicality(hash, Some(hash)), CanonicalityStatus::Canonical);
        assert_eq!(
            derive_canonicality(hash, Some(B256::repeat_byte(2))),
            CanonicalityStatus::NonCanonical
        );
        assert_eq!(derive_canonicality(hash, None), CanonicalityStatus::Unknown);
    }
}
