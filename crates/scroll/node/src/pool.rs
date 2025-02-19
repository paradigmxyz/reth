use alloy_consensus::{constants::EIP1559_TX_TYPE_ID, Transaction, Typed2718};
use alloy_eips::{
    eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M,
    eip2718::Encodable2718,
    eip2930::AccessList,
    eip4844::{BlobAndProofV1, BlobTransactionSidecar, BlobTransactionValidationError},
    eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, Bytes, ChainId, TxHash, TxKind, B256, U256};
use reth_eth_wire_types::HandleMempoolData;
use reth_primitives::{kzg::KzgSettings, Recovered};
use reth_primitives_traits::{
    transaction::error::TryFromRecoveredTransactionError, SignedTransaction,
};
use reth_scroll_primitives::ScrollTransactionSigned;
use reth_transaction_pool::{
    error::PoolError, AllPoolTransactions, AllTransactionsEvents, BestTransactions,
    BestTransactionsAttributes, BlobStoreError, BlockInfo, EthBlobTransactionSidecar,
    EthPoolTransaction, EthPooledTransaction, GetPooledTransactionLimit, NewBlobSidecar,
    NewTransactionEvent, PoolResult, PoolSize, PoolTransaction, PropagatedTransactions,
    TransactionEvents, TransactionListenerKind, TransactionOrigin, TransactionPool,
    ValidPoolTransaction,
};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{mpsc, mpsc::Receiver};

/// A [`TransactionPool`] implementation that does nothing for Scroll.
///
/// All transactions are rejected and no events are emitted.
/// This type will never hold any transactions and is only useful for wiring components together
/// using the Scroll primitive types.
#[derive(Debug, Clone, Default)]
pub struct ScrollNoopTransactionPool;

impl TransactionPool for ScrollNoopTransactionPool {
    type Transaction = ScrollPooledTransaction;

    fn pool_size(&self) -> PoolSize {
        Default::default()
    }

    fn block_info(&self) -> BlockInfo {
        BlockInfo {
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
            last_seen_block_hash: Default::default(),
            last_seen_block_number: 0,
            pending_basefee: 0,
            pending_blob_fee: None,
        }
    }

    async fn add_transaction_and_subscribe(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents> {
        let hash = *transaction.hash();
        Err(PoolError::other(hash, Box::new(NoopInsertError::new(transaction))))
    }

    async fn add_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        let hash = *transaction.hash();
        Err(PoolError::other(hash, Box::new(NoopInsertError::new(transaction))))
    }

    async fn add_transactions(
        &self,
        _origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> Vec<PoolResult<TxHash>> {
        transactions
            .into_iter()
            .map(|transaction| {
                let hash = *transaction.hash();
                Err(PoolError::other(hash, Box::new(NoopInsertError::new(transaction))))
            })
            .collect()
    }

    fn transaction_event_listener(&self, _tx_hash: TxHash) -> Option<TransactionEvents> {
        None
    }

    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction> {
        AllTransactionsEvents::new(mpsc::channel(1).1)
    }

    fn pending_transactions_listener_for(
        &self,
        _kind: TransactionListenerKind,
    ) -> Receiver<TxHash> {
        mpsc::channel(1).1
    }

    fn new_transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        mpsc::channel(1).1
    }

    fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar> {
        mpsc::channel(1).1
    }

    fn new_transactions_listener_for(
        &self,
        _kind: TransactionListenerKind,
    ) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        mpsc::channel(1).1
    }

    fn pooled_transaction_hashes(&self) -> Vec<TxHash> {
        vec![]
    }

    fn pooled_transaction_hashes_max(&self, _max: usize) -> Vec<TxHash> {
        vec![]
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn pooled_transactions_max(
        &self,
        _max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_pooled_transaction_elements(
        &self,
        _tx_hashes: Vec<TxHash>,
        _limit: GetPooledTransactionLimit,
    ) -> Vec<<Self::Transaction as PoolTransaction>::Pooled> {
        vec![]
    }

    fn get_pooled_transaction_element(
        &self,
        _tx_hash: TxHash,
    ) -> Option<Recovered<<Self::Transaction as PoolTransaction>::Pooled>> {
        None
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(std::iter::empty())
    }

    fn best_transactions_with_attributes(
        &self,
        _: BestTransactionsAttributes,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(std::iter::empty())
    }

    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn pending_transactions_max(
        &self,
        _max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        AllPoolTransactions::default()
    }

    fn remove_transactions(
        &self,
        _hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn remove_transactions_and_descendants(
        &self,
        _hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn remove_transactions_by_sender(
        &self,
        _sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn retain_unknown<A>(&self, _announcement: &mut A)
    where
        A: HandleMempoolData,
    {
    }

    fn get(&self, _tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        None
    }

    fn get_all(&self, _txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn on_propagated(&self, _txs: PropagatedTransactions) {}

    fn get_transactions_by_sender(
        &self,
        _sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_pending_transactions_with_predicate(
        &self,
        _predicate: impl FnMut(&ValidPoolTransaction<Self::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_pending_transactions_by_sender(
        &self,
        _sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_queued_transactions_by_sender(
        &self,
        _sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_highest_transaction_by_sender(
        &self,
        _sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        None
    }

    fn get_highest_consecutive_transaction_by_sender(
        &self,
        _sender: Address,
        _on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        None
    }

    fn get_transaction_by_sender_and_nonce(
        &self,
        _sender: Address,
        _nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        None
    }

    fn get_transactions_by_origin(
        &self,
        _origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn get_pending_transactions_by_origin(
        &self,
        _origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn unique_senders(&self) -> HashSet<Address> {
        Default::default()
    }

    fn get_blob(
        &self,
        _tx_hash: TxHash,
    ) -> Result<Option<Arc<BlobTransactionSidecar>>, BlobStoreError> {
        Ok(None)
    }

    fn get_all_blobs(
        &self,
        _tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<(TxHash, Arc<BlobTransactionSidecar>)>, BlobStoreError> {
        Ok(vec![])
    }

    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<Arc<BlobTransactionSidecar>>, BlobStoreError> {
        if tx_hashes.is_empty() {
            return Ok(vec![]);
        }
        Err(BlobStoreError::MissingSidecar(tx_hashes[0]))
    }

    fn get_blobs_for_versioned_hashes(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError> {
        Ok(vec![None; versioned_hashes.len()])
    }
}

/// A transaction that can be included in the [`ScrollNoopTransactionPool`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScrollPooledTransaction(EthPooledTransaction<ScrollTransactionSigned>);

impl ScrollPooledTransaction {
    /// Returns a new [`ScrollPooledTransaction`].
    pub fn new(transaction: Recovered<ScrollTransactionSigned>, encoded_length: usize) -> Self {
        Self(EthPooledTransaction::new(transaction, encoded_length))
    }
}

impl TryFrom<Recovered<ScrollTransactionSigned>> for ScrollPooledTransaction {
    type Error = TryFromRecoveredTransactionError;

    fn try_from(tx: Recovered<ScrollTransactionSigned>) -> Result<Self, Self::Error> {
        // ensure we can handle the transaction type and its format
        match tx.ty() {
            0..=EIP1559_TX_TYPE_ID => {
                // supported
            }
            unsupported => {
                // unsupported transaction type
                return Err(TryFromRecoveredTransactionError::UnsupportedTransactionType(
                    unsupported,
                ))
            }
        };

        let encoded_length = tx.encode_2718_len();
        let transaction = Self::new(tx, encoded_length);
        Ok(transaction)
    }
}

impl From<ScrollPooledTransaction> for Recovered<ScrollTransactionSigned> {
    fn from(tx: ScrollPooledTransaction) -> Self {
        tx.0.transaction
    }
}

impl From<Recovered<scroll_alloy_consensus::ScrollPooledTransaction>> for ScrollPooledTransaction {
    fn from(tx: Recovered<scroll_alloy_consensus::ScrollPooledTransaction>) -> Self {
        let encoded_length = tx.encode_2718_len();
        let (tx, signer) = tx.into_parts();
        // no blob sidecar
        let tx = Recovered::new_unchecked(tx.into(), signer);
        Self::new(tx, encoded_length)
    }
}

impl PoolTransaction for ScrollPooledTransaction {
    type TryFromConsensusError = TryFromRecoveredTransactionError;

    type Consensus = ScrollTransactionSigned;

    type Pooled = scroll_alloy_consensus::ScrollPooledTransaction;

    fn clone_into_consensus(&self) -> Recovered<Self::Consensus> {
        self.0.transaction().clone()
    }

    fn into_consensus(self) -> Recovered<Self::Consensus> {
        self.0.transaction
    }

    fn from_pooled(tx: Recovered<Self::Pooled>) -> Self {
        let encoded_len = tx.encode_2718_len();
        let tx = tx.map_transaction(|tx| tx.into());
        Self::new(tx, encoded_len)
    }

    /// Returns hash of the transaction.
    fn hash(&self) -> &TxHash {
        self.0.transaction.tx_hash()
    }

    /// Returns the Sender of the transaction.
    fn sender(&self) -> Address {
        self.0.transaction.signer()
    }

    /// Returns a reference to the Sender of the transaction.
    fn sender_ref(&self) -> &Address {
        self.0.transaction.signer_ref()
    }

    /// Returns the cost that this transaction is allowed to consume:
    ///
    /// For EIP-1559 transactions: `max_fee_per_gas * gas_limit + tx_value`.
    /// For legacy transactions: `gas_price * gas_limit + tx_value`.
    /// For EIP-4844 blob transactions: `max_fee_per_gas * gas_limit + tx_value +
    /// max_blob_fee_per_gas * blob_gas_used`.
    fn cost(&self) -> &U256 {
        &self.0.cost
    }

    /// Returns the length of the rlp encoded object
    fn encoded_length(&self) -> usize {
        self.0.encoded_length
    }
}

impl reth_primitives_traits::InMemorySize for ScrollPooledTransaction {
    fn size(&self) -> usize {
        self.0.size()
    }
}

impl Typed2718 for ScrollPooledTransaction {
    fn ty(&self) -> u8 {
        self.0.ty()
    }
}

impl Transaction for ScrollPooledTransaction {
    fn chain_id(&self) -> Option<ChainId> {
        self.0.chain_id()
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.0.gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.0.gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.0.max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.0.max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.0.max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.0.priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.0.effective_gas_price(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.0.is_dynamic_fee()
    }

    fn kind(&self) -> TxKind {
        self.0.kind()
    }

    fn is_create(&self) -> bool {
        self.0.is_create()
    }

    fn value(&self) -> U256 {
        self.0.value()
    }

    fn input(&self) -> &Bytes {
        self.0.input()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.0.access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.0.blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.0.authorization_list()
    }
}

impl EthPoolTransaction for ScrollPooledTransaction {
    fn take_blob(&mut self) -> EthBlobTransactionSidecar {
        EthBlobTransactionSidecar::None
    }

    fn try_into_pooled_eip4844(
        self,
        _sidecar: Arc<BlobTransactionSidecar>,
    ) -> Option<Recovered<Self::Pooled>> {
        None
    }

    fn try_from_eip4844(
        _tx: Recovered<Self::Consensus>,
        _sidecar: BlobTransactionSidecar,
    ) -> Option<Self> {
        None
    }

    fn validate_blob(
        &self,
        _blob: &BlobTransactionSidecar,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(self.ty()))
    }
}

/// An error that contains the transaction that failed to be inserted into the noop pool.
#[derive(Debug, Clone, thiserror::Error)]
#[error("can't insert transaction into the noop pool that does nothing")]
struct NoopInsertError {
    tx: ScrollPooledTransaction,
}

impl NoopInsertError {
    const fn new(tx: ScrollPooledTransaction) -> Self {
        Self { tx }
    }
}
