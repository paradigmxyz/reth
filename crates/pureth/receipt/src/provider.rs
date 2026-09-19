use crate::{
    convert_receipts, ReceiptConversionError, ReceiptSnapshot, TreeConstructionError,
    RECEIPT_SCHEMA_ID,
};
use alloy_consensus::{TxLegacy, TxType};
use alloy_primitives::{Address, Bytes, Log, Signature, TxKind, B256};
use reth_ethereum_primitives::{
    Block, BlockBody, EthPrimitives, Receipt, Transaction as EthereumTransaction, TransactionSigned,
};
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    BlockReader, ProviderError, ReceiptProvider, TransactionVariant,
};

pub const DETERMINISTIC_PRODUCER_REVISION: &str = "deterministic-receipt-provider-v0";
pub const RETH_HISTORICAL_PRODUCER_REVISION: &str = "reth-historical-receipt-provider-v0";
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
        schema_id: &str,
    ) -> Result<&ProviderSnapshot, LookupError> {
        if object != ObjectKind::Receipts {
            return Err(LookupError::UnsupportedObject);
        }
        if schema_id != RECEIPT_SCHEMA_ID {
            return Err(LookupError::UnsupportedSchema);
        }

        self.snapshots
            .iter()
            .find(|snapshot| snapshot.block_hash == block_hash)
            .ok_or(LookupError::UnknownBlock)
    }
}

#[derive(Debug)]
pub struct ProviderSnapshot {
    block_hash: B256,
    block_status: CanonicalityStatus,
    object: ObjectKind,
    schema_id: &'static str,
    root_context: RootContext,
    producer_revision: &'static str,
    receipt_snapshot: ReceiptSnapshot,
}

impl ProviderSnapshot {
    pub fn from_reth_historical<N>(
        provider: &BlockchainProvider<N>,
        block_hash: B256,
    ) -> Result<Self, HistoricalAcquisitionError>
    where
        N: ProviderNodeTypes<Primitives = EthPrimitives>,
    {
        let (block, receipts) = {
            let view = provider
                .consistent_provider()
                .map_err(HistoricalAcquisitionError::ConsistentView)?;
            let block = view
                .recovered_block(block_hash.into(), TransactionVariant::WithHash)
                .map_err(HistoricalAcquisitionError::BlockRead)?
                .ok_or(HistoricalAcquisitionError::BlockUnavailable)?;
            let receipts = view
                .receipts_by_block(block_hash.into())
                .map_err(HistoricalAcquisitionError::ReceiptsRead)?
                .ok_or(HistoricalAcquisitionError::ReceiptsUnavailable)?;
            (block, receipts)
        };

        let receipts = convert_receipts(&block, &receipts)
            .map_err(HistoricalAcquisitionError::ReceiptConversion)?;
        let receipt_snapshot = ReceiptSnapshot::build(receipts)
            .map_err(HistoricalAcquisitionError::TreeConstruction)?;

        Ok(Self {
            block_hash,
            block_status: CanonicalityStatus::Canonical,
            object: ObjectKind::Receipts,
            schema_id: RECEIPT_SCHEMA_ID,
            root_context: RootContext::RethExperimentalUnanchored,
            producer_revision: RETH_HISTORICAL_PRODUCER_REVISION,
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

    pub const fn schema_id(&self) -> &'static str {
        self.schema_id
    }

    pub const fn root_context(&self) -> RootContext {
        self.root_context
    }

    pub const fn producer_revision(&self) -> &'static str {
        self.producer_revision
    }

    pub const fn receipt_snapshot(&self) -> &ReceiptSnapshot {
        &self.receipt_snapshot
    }

    pub const fn root(&self) -> B256 {
        self.receipt_snapshot.root()
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderBuildError {
    RecoveredBlock,
    ReceiptConversion(ReceiptConversionError),
    TreeConstruction(TreeConstructionError),
}

#[derive(Debug)]
pub enum HistoricalAcquisitionError {
    ConsistentView(ProviderError),
    BlockRead(ProviderError),
    ReceiptsRead(ProviderError),
    BlockUnavailable,
    ReceiptsUnavailable,
    ReceiptConversion(ReceiptConversionError),
    TreeConstruction(TreeConstructionError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupError {
    UnknownBlock,
    UnsupportedObject,
    UnsupportedSchema,
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
    receipts: Vec<Receipt>,
) -> Result<ProviderSnapshot, ProviderBuildError> {
    let senders = vec![Address::repeat_byte(0x66); transactions.len()];
    let block = RecoveredBlock::try_new_unhashed(
        Block {
            header: Default::default(),
            body: BlockBody { transactions, ..Default::default() },
        },
        senders,
    )
    .map_err(|_| ProviderBuildError::RecoveredBlock)?;
    let receipts =
        convert_receipts(&block, &receipts).map_err(ProviderBuildError::ReceiptConversion)?;
    let receipt_snapshot =
        ReceiptSnapshot::build(receipts).map_err(ProviderBuildError::TreeConstruction)?;

    Ok(ProviderSnapshot {
        block_hash,
        block_status: CanonicalityStatus::Unknown,
        object: ObjectKind::Receipts,
        schema_id: RECEIPT_SCHEMA_ID,
        root_context: RootContext::DeterministicTestData,
        producer_revision: DETERMINISTIC_PRODUCER_REVISION,
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

const fn receipt(cumulative_gas_used: u64, logs: Vec<Log>) -> Receipt {
    Receipt { tx_type: TxType::Legacy, success: true, cumulative_gas_used, logs }
}

fn log(address: Address, topics: Vec<B256>, data: &[u8]) -> Log {
    Log::new_unchecked(address, topics, Bytes::copy_from_slice(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, DBProvider, DatabaseProviderFactory, ExecutionOutcome, OriginalValuesKnown,
        StateWriteConfig, StateWriter,
    };

    fn historical_provider(
        receipts: Option<Vec<Receipt>>,
    ) -> (BlockchainProvider<MockNodeTypesWithDB>, B256) {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        let block = RecoveredBlock::try_new_unhashed(
            Block {
                header: Default::default(),
                body: BlockBody { transactions: vec![transaction(0)], ..Default::default() },
            },
            vec![Address::repeat_byte(0x66)],
        )
        .unwrap();
        let block_hash = block.hash();
        provider.insert_block(&block).unwrap();
        if let Some(receipts) = receipts {
            provider
                .write_state(
                    &ExecutionOutcome {
                        first_block: 0,
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

    fn singleton_receipts() -> Vec<Receipt> {
        vec![receipt(
            21_000,
            vec![log(Address::repeat_byte(0x11), vec![B256::repeat_byte(0x22)], &[1, 2, 3])],
        )]
    }

    #[test]
    fn historical_provider_returns_a_coherent_snapshot() {
        let (provider, block_hash) = historical_provider(Some(singleton_receipts()));
        let result = ProviderSnapshot::from_reth_historical(&provider, block_hash).unwrap();
        let snapshot = result.receipt_snapshot();

        assert_eq!(result.block_hash(), block_hash);
        assert_eq!(result.block_status(), CanonicalityStatus::Canonical);
        assert_eq!(result.object(), ObjectKind::Receipts);
        assert_eq!(result.schema_id(), RECEIPT_SCHEMA_ID);
        assert_eq!(result.root_context(), RootContext::RethExperimentalUnanchored);
        assert_eq!(result.producer_revision(), RETH_HISTORICAL_PRODUCER_REVISION);
        assert_eq!(snapshot.receipts().len(), 1);
        assert_eq!(
            snapshot.receipts().get(0).unwrap().logs()[0].address(),
            Address::repeat_byte(0x11)
        );
        assert_eq!(
            result.root(),
            alloy_primitives::b256!(
                "5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
            )
        );
        assert_eq!(result.root(), snapshot.root());
        assert_eq!(result.root(), snapshot.tree().root());
    }

    #[test]
    fn historical_provider_distinguishes_missing_blocks() {
        let (provider, _) = historical_provider(Some(singleton_receipts()));

        let result = ProviderSnapshot::from_reth_historical(&provider, B256::repeat_byte(0xff));
        assert!(matches!(result, Err(HistoricalAcquisitionError::BlockUnavailable)));
    }

    #[test]
    fn historical_provider_distinguishes_missing_receipts() {
        let (provider, block_hash) = historical_provider(None);

        let result = ProviderSnapshot::from_reth_historical(&provider, block_hash);
        assert!(matches!(result, Err(HistoricalAcquisitionError::ReceiptsUnavailable)));
    }

    #[test]
    fn historical_provider_preserves_conversion_errors() {
        let receipts = vec![Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 21_000,
            logs: Vec::new(),
        }];
        let (provider, block_hash) = historical_provider(Some(receipts));

        let result = ProviderSnapshot::from_reth_historical(&provider, block_hash);
        assert!(matches!(
            result,
            Err(HistoricalAcquisitionError::ReceiptConversion(
                ReceiptConversionError::TransactionTypeMismatch {
                    index: 0,
                    transaction,
                    receipt,
                }
            )) if transaction == TxType::Legacy as u8 && receipt == TxType::Eip1559 as u8
        ));
    }

    #[test]
    fn known_blocks_return_their_coherent_snapshots() {
        let provider = DeterministicProvider::new().unwrap();

        for (block_hash, receipt_index, log_index, receipt_count, address, root) in [
            (
                SINGLETON_BLOCK_HASH,
                0,
                0,
                1,
                Address::repeat_byte(0x11),
                alloy_primitives::b256!(
                    "5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
                ),
            ),
            (
                MULTIPLE_LOGS_BLOCK_HASH,
                0,
                1,
                1,
                Address::repeat_byte(0x33),
                alloy_primitives::b256!(
                    "d4e90213f6f7fa76997b8d2a3e56c1df70c7846a06e8deeead604844b0cfa43a"
                ),
            ),
            (
                PROGRESSIVE_RECEIPTS_BLOCK_HASH,
                5,
                0,
                6,
                Address::repeat_byte(0x16),
                alloy_primitives::b256!(
                    "a8d13e4ec4c2b516ebd5b536f94784667c0098c5e1d6017453313cea532c1830"
                ),
            ),
        ] {
            let result =
                provider.lookup(block_hash, ObjectKind::Receipts, RECEIPT_SCHEMA_ID).unwrap();
            let snapshot = result.receipt_snapshot();

            assert_eq!(result.block_hash(), block_hash);
            assert_eq!(result.block_status(), CanonicalityStatus::Unknown);
            assert_eq!(result.object(), ObjectKind::Receipts);
            assert_eq!(result.schema_id(), RECEIPT_SCHEMA_ID);
            assert_eq!(result.root_context(), RootContext::DeterministicTestData);
            assert_eq!(result.producer_revision(), DETERMINISTIC_PRODUCER_REVISION);
            assert_eq!(snapshot.receipts().len(), receipt_count);
            assert_eq!(
                snapshot.receipts().get(receipt_index).unwrap().logs()[log_index].address(),
                address
            );
            assert_eq!(result.root(), root);
            assert_eq!(result.root(), snapshot.root());
            assert_eq!(result.root(), snapshot.tree().root());
        }
    }

    #[test]
    fn repeated_lookups_return_the_same_stored_snapshots() {
        let provider = DeterministicProvider::new().unwrap();

        for block_hash in
            [SINGLETON_BLOCK_HASH, MULTIPLE_LOGS_BLOCK_HASH, PROGRESSIVE_RECEIPTS_BLOCK_HASH]
        {
            let first =
                provider.lookup(block_hash, ObjectKind::Receipts, RECEIPT_SCHEMA_ID).unwrap();
            let second =
                provider.lookup(block_hash, ObjectKind::Receipts, RECEIPT_SCHEMA_ID).unwrap();

            assert!(std::ptr::eq(first, second));
            assert!(std::ptr::eq(first.receipt_snapshot(), second.receipt_snapshot()));
            assert_eq!(first.root(), second.root());
        }
    }

    #[test]
    fn deterministic_blocks_remain_distinct() {
        let provider = DeterministicProvider::new().unwrap();
        let snapshots =
            [SINGLETON_BLOCK_HASH, MULTIPLE_LOGS_BLOCK_HASH, PROGRESSIVE_RECEIPTS_BLOCK_HASH].map(
                |block_hash| {
                    provider.lookup(block_hash, ObjectKind::Receipts, RECEIPT_SCHEMA_ID).unwrap()
                },
            );

        for left in 0..snapshots.len() {
            for right in left + 1..snapshots.len() {
                assert_ne!(snapshots[left].block_hash(), snapshots[right].block_hash());
                assert_ne!(snapshots[left].root(), snapshots[right].root());
                assert!(!std::ptr::eq(snapshots[left], snapshots[right]));
            }
        }
    }

    #[test]
    fn lookup_errors_follow_object_schema_block_precedence() {
        let provider = DeterministicProvider::new().unwrap();
        let unknown_block = B256::repeat_byte(0xff);

        assert!(matches!(
            provider.lookup(unknown_block, ObjectKind::Withdrawals, "other-schema"),
            Err(LookupError::UnsupportedObject)
        ));
        assert!(matches!(
            provider.lookup(unknown_block, ObjectKind::Receipts, "other-schema"),
            Err(LookupError::UnsupportedSchema)
        ));
        assert!(matches!(
            provider.lookup(unknown_block, ObjectKind::Receipts, RECEIPT_SCHEMA_ID),
            Err(LookupError::UnknownBlock)
        ));
        assert!(matches!(
            provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Withdrawals, RECEIPT_SCHEMA_ID),
            Err(LookupError::UnsupportedObject)
        ));
        assert!(matches!(
            provider.lookup(SINGLETON_BLOCK_HASH, ObjectKind::Receipts, "other-schema"),
            Err(LookupError::UnsupportedSchema)
        ));
    }
}
