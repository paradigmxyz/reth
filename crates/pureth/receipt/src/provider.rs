use crate::eip6466::{Eip6466ReceiptSnapshot, Eip6466SnapshotError};
use alloy_consensus::BlockHeader as _;
use alloy_primitives::B256;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_ethereum_primitives::EthPrimitives;
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    BlockHashReader, BlockNumReader, BlockReader, ProviderError, ReceiptProvider,
    TransactionVariant,
};
use std::fmt;

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
            let head =
                view.best_block_number().map_err(HistoricalAcquisitionError::CanonicalHeadRead)?;
            let block_status = if block.number() > head {
                CanonicalityStatus::NonCanonical
            } else {
                let canonical_hash = view
                    .block_hash(block.number())
                    .map_err(HistoricalAcquisitionError::CanonicalHashRead)?;
                derive_canonicality(block_hash, canonical_hash)
            };
            let receipts = view
                .receipts_by_block(block_hash.into())
                .map_err(HistoricalAcquisitionError::ReceiptsRead)?
                .ok_or(HistoricalAcquisitionError::ReceiptsUnavailable)?;
            (block, block_status, receipts)
        };

        let receipt_snapshot =
            Eip6466ReceiptSnapshot::from_block(&block, &receipts, provider.chain_spec().as_ref())
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

    pub fn into_receipt_snapshot(self) -> Eip6466ReceiptSnapshot {
        self.receipt_snapshot
    }

    pub fn root(&self) -> B256 {
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
    RethExperimentalUnanchored,
}

#[derive(Debug)]
pub enum HistoricalAcquisitionError {
    ConsistentView(ProviderError),
    BlockRead(ProviderError),
    CanonicalHeadRead(ProviderError),
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
            Self::ConsistentView(_) | Self::CanonicalHeadRead(_) | Self::CanonicalHashRead(_) => {
                false
            }
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
            Self::CanonicalHeadRead(error) => {
                write!(formatter, "canonical head read failed: {error}")
            }
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
            Self::CanonicalHeadRead(error) |
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

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use alloy_consensus::{Header, TxLegacy, TxType};
    use alloy_primitives::{Address, Bytes, Log as ExecutionLog, Signature, TxKind};
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_ethereum_primitives::{
        Block, BlockBody, Receipt as StoredReceipt, Transaction as EthereumTransaction,
        TransactionSigned,
    };
    use reth_primitives_traits::RecoveredBlock;
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

    pub(super) fn transaction(nonce: u64) -> TransactionSigned {
        TransactionSigned::new_unhashed(
            EthereumTransaction::Legacy(TxLegacy {
                nonce,
                gas_limit: 21_000,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReceiptConstructionError, Receipts, TreeConstructionError};
    use alloy_consensus::{Header, TxType};
    use alloy_primitives::Address;
    use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_primitives::{Block, BlockBody, Receipt as StoredReceipt};
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::{
        BlockExecutionOutput, BlockWriter, DBProvider, DatabaseProviderFactory, ExecutionOutcome,
        OriginalValuesKnown, StateWriteConfig, StateWriter,
    };
    use std::{error::Error, sync::Arc};

    #[test]
    fn historical_snapshot_transfers_the_original_receipt_tree_without_reconstruction() {
        let (provider, block_hash) =
            test_utils::historical_provider(Some(test_utils::singleton_receipts()));
        let snapshot =
            RethRootProvider::new(provider).lookup(block_hash, ObjectKind::Receipts).unwrap();
        let source_hash = snapshot.block_hash();
        let status = snapshot.block_status();
        let context = snapshot.root_context();
        let root = snapshot.root();
        let children = snapshot.receipt_snapshot().tree().children().unwrap().as_ptr();
        let receipts = snapshot.into_receipt_snapshot();
        assert_eq!(source_hash, block_hash);
        assert_eq!(status, CanonicalityStatus::Canonical);
        assert_eq!(context, RootContext::RethExperimentalUnanchored);
        assert_eq!(receipts.root(), root);
        assert_eq!(receipts.tree().children().unwrap().as_ptr(), children);
        assert_eq!(receipts.receipts().len(), 1);
    }

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
    fn historical_cases_preserve_distinct_receipts_and_coherent_snapshots() {
        let mut roots = Vec::new();
        for (count, receipt_index, log_index, address) in [
            (1usize, 0, 0, Address::repeat_byte(0x11)),
            (1, 0, 1, Address::repeat_byte(0x33)),
            (6, 5, 0, Address::repeat_byte(0x16)),
        ] {
            let receipts = (0..count)
                .map(|index| {
                    let mut receipt = test_utils::singleton_receipts().remove(0);
                    receipt.cumulative_gas_used = 21_000 * (index as u64 + 1);
                    receipt.logs[0].address = Address::repeat_byte(0x11 + index as u8);
                    if log_index == 1 {
                        let mut log = receipt.logs[0].clone();
                        log.address = address;
                        receipt.logs.push(log);
                    }
                    receipt
                })
                .collect::<Vec<_>>();
            let block = RecoveredBlock::try_new_unhashed(
                Block {
                    header: Header {
                        gas_used: 21_000 * count as u64,
                        gas_limit: 30_000_000,
                        ..Default::default()
                    },
                    body: BlockBody {
                        transactions: (0..count)
                            .map(|index| test_utils::transaction(index as u64))
                            .collect(),
                        ..Default::default()
                    },
                },
                vec![Address::repeat_byte(0x66); count],
            )
            .unwrap();
            let (provider, block_hash) = test_utils::historical_provider_for_block(
                block,
                Some(receipts),
                Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build()),
            );
            let provider = RethRootProvider::new(provider);
            let first = provider.lookup(block_hash, ObjectKind::Receipts).unwrap();
            let second = provider.lookup(block_hash, ObjectKind::Receipts).unwrap();
            let snapshot = first.receipt_snapshot();
            assert_eq!(first.root(), second.root());
            assert_eq!(snapshot.serialized(), second.receipt_snapshot().serialized());
            assert_eq!(first.block_hash(), block_hash);
            assert_eq!(first.block_status(), CanonicalityStatus::Canonical);
            assert_eq!(first.object(), ObjectKind::Receipts);
            assert_eq!(first.root_context(), RootContext::RethExperimentalUnanchored);
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
        let (provider, block_hash) =
            test_utils::historical_provider(Some(test_utils::singleton_receipts()));
        let provider = RethRootProvider::new(provider);
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Withdrawals),
            Err(RethRootProviderError::Lookup(LookupError::UnsupportedObject))
        ));
        assert!(matches!(
            provider.lookup(B256::ZERO, ObjectKind::Receipts),
            Err(RethRootProviderError::Lookup(LookupError::UnknownBlock))
        ));
        assert!(matches!(
            provider.lookup(block_hash, ObjectKind::Withdrawals),
            Err(RethRootProviderError::Lookup(LookupError::UnsupportedObject))
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

        let error = HistoricalAcquisitionError::Snapshot(Eip6466SnapshotError::Tree(
            TreeConstructionError::InvalidWidth { width: 0 },
        ));
        assert!(error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<TreeConstructionError>()
            .is_some());
        assert!(HistoricalAcquisitionError::BlockUnavailable.source().is_none());
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

    #[test]
    fn historical_canonicality_rejects_reorged_blocks_while_disk_cleanup_is_pending() {
        let (provider, ancestor) =
            test_utils::historical_provider(Some(test_utils::singleton_receipts()));
        let base = provider
            .recovered_block(ancestor.into(), TransactionVariant::WithHash)
            .unwrap()
            .unwrap();
        let database = provider.database_provider_rw().unwrap();
        let mut parent = ancestor;
        let old = (1..=2)
            .map(|number| {
                let block = RecoveredBlock::try_new_unhashed(
                    Block {
                        header: Header { number, parent_hash: parent, ..base.header().clone() },
                        body: base.body().clone(),
                    },
                    base.senders().to_vec(),
                )
                .unwrap();
                database.insert_block(&block).unwrap();
                parent = block.hash();
                block
            })
            .collect::<Vec<_>>();
        database
            .write_state(
                &ExecutionOutcome {
                    first_block: 1,
                    receipts: vec![test_utils::singleton_receipts(); 2],
                    ..Default::default()
                },
                OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        database.commit().unwrap();
        let executed = old
            .iter()
            .map(|block| {
                ExecutedBlock::new(
                    Arc::new(block.clone()),
                    Arc::new(BlockExecutionOutput {
                        result: reth_provider::BlockExecutionResult {
                            receipts: test_utils::singleton_receipts(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    Default::default(),
                )
            })
            .collect::<Vec<_>>();
        let memory = provider.canonical_in_memory_state();
        memory.update_chain(NewCanonicalChain::Commit { new: executed.clone() });
        let captured = provider.consistent_provider().unwrap();
        let before = ProviderSnapshot::from_reth_historical(&provider, old[1].hash()).unwrap();
        let root = before.root();
        let bytes = before.receipt_snapshot().serialized().to_vec();
        let replacement = RecoveredBlock::try_new_unhashed(
            Block {
                header: Header { extra_data: vec![1].into(), ..old[0].header().clone() },
                body: old[0].body().clone(),
            },
            old[0].senders().to_vec(),
        )
        .unwrap();
        let replacement_hash = replacement.hash();
        memory.update_chain(NewCanonicalChain::Reorg {
            old: executed.clone(),
            new: vec![ExecutedBlock::new(
                Arc::new(replacement),
                Arc::new(executed[0].execution_outcome().clone()),
                Default::default(),
            )],
        });
        assert_eq!(captured.best_block_number().unwrap(), 2);
        assert_eq!(captured.block_hash(1).unwrap(), Some(old[0].hash()));
        assert_eq!(captured.block_hash(2).unwrap(), Some(old[1].hash()));
        let current = provider.consistent_provider().unwrap();
        assert_eq!(current.best_block_number().unwrap(), 1);
        assert_eq!(current.block_hash(1).unwrap(), Some(replacement_hash));
        assert_eq!(current.block_hash(2).unwrap(), Some(old[1].hash()));
        for block in &old {
            let snapshot = ProviderSnapshot::from_reth_historical(&provider, block.hash()).unwrap();
            assert_eq!(snapshot.block_status(), CanonicalityStatus::NonCanonical);
        }
        assert_eq!(
            ProviderSnapshot::from_reth_historical(&provider, replacement_hash)
                .unwrap()
                .block_status(),
            CanonicalityStatus::Canonical
        );
        assert_eq!(before.block_status(), CanonicalityStatus::Canonical);
        assert_eq!(before.root(), root);
        assert_eq!(before.receipt_snapshot().serialized(), bytes.as_slice());
        let error =
            HistoricalAcquisitionError::CanonicalHeadRead(ProviderError::InvalidStorageOutput);
        assert!(!error.is_unavailable());
        assert!(error.source().unwrap().downcast_ref::<ProviderError>().is_some());
    }
}
