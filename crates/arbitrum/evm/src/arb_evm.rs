extern crate alloc;

use alloy_evm::{
    eth::EthEvmContext, precompiles::PrecompilesMap, Database, EvmEnv, EvmFactory, IntoTxEnv,
};
use alloy_evm::tx::{FromRecoveredTx, FromTxWithEncoded};
use alloy_primitives::{Address, Bytes};
use core::fmt::Debug;
use reth_arbitrum_primitives::ArbTransactionSigned;
use revm::context::TxEnv;
use revm::inspector::NoOpInspector;
use revm::context::result::EVMError;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArbTransaction<T>(pub T);

impl FromRecoveredTx<ArbTransactionSigned> for ArbTransaction<TxEnv> {
    fn from_recovered_tx(signed: &ArbTransactionSigned, sender: Address) -> Self {
        use alloy_consensus::Transaction as _;
        let mut tx = TxEnv::default();
        let kind = match signed.kind() {
            alloy_primitives::TxKind::Create => revm::primitives::TxKind::Create,
            alloy_primitives::TxKind::Call(to) => revm::primitives::TxKind::Call(to),
        };
        tx.caller = sender;
        tx.gas_limit = signed.gas_limit();
        if (matches!(signed.tx_type(), reth_arbitrum_primitives::ArbTxType::Internal)
            || matches!(signed.tx_type(), reth_arbitrum_primitives::ArbTxType::Deposit)) && tx.gas_limit == 0
        {
            tx.gas_limit = 1_000_000;
        }
        tx.gas_priority_fee = Some(0);
        match signed.tx_type() {
            reth_arbitrum_primitives::ArbTxType::Legacy => {
                tx.value = signed.value();
                tx.gas_price = signed.max_fee_per_gas();
            }
            reth_arbitrum_primitives::ArbTxType::Deposit
            | reth_arbitrum_primitives::ArbTxType::Internal => {
                tx.value = alloy_primitives::U256::ZERO;
                tx.gas_price = 0;
            }
            _ => {
                tx.value = alloy_primitives::U256::ZERO;
                tx.gas_price = signed.max_fee_per_gas();
            }
        }
        tx.kind = kind;
        tx.data = signed.input().clone();
        
        let should_skip_checks = matches!(
            signed.tx_type(),
            reth_arbitrum_primitives::ArbTxType::Internal
                | reth_arbitrum_primitives::ArbTxType::Deposit
                | reth_arbitrum_primitives::ArbTxType::Contract
                | reth_arbitrum_primitives::ArbTxType::Retry
                | reth_arbitrum_primitives::ArbTxType::SubmitRetryable
        );
        
        if should_skip_checks {
            tx.chain_id = Some(421614);
        } else {
            tx.chain_id = Some(signed.chain_id().unwrap_or_default());
        }
        
        tx.nonce = signed.nonce();
        tx.access_list = signed.access_list().cloned().unwrap_or_default();
        tx.blob_hashes = signed.blob_versioned_hashes().unwrap_or(&[]).to_vec();
        tx.authorization_list = signed
            .authorization_list()
            .unwrap_or(&[])
            .iter()
            .cloned()
            .map(alloy_consensus::transaction::Either::Left)
            .collect();
        tx.max_fee_per_blob_gas = signed.max_fee_per_blob_gas().unwrap_or_default();
        ArbTransaction(tx)
    }
}

impl FromTxWithEncoded<ArbTransactionSigned> for ArbTransaction<TxEnv> {
    fn from_encoded_tx(signed: &ArbTransactionSigned, sender: Address, _encoded: Bytes) -> Self {
        <ArbTransaction<TxEnv> as FromRecoveredTx<ArbTransactionSigned>>::from_recovered_tx(signed, sender)
    }
}

impl reth_evm::TransactionEnv for ArbTransaction<TxEnv> {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.0.set_gas_limit(gas_limit);
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.0.set_nonce(nonce);
    }

    fn set_access_list(&mut self, access_list: alloy_eips::eip2930::AccessList) {
        self.0.set_access_list(access_list);
    }

    fn set_gas_price(&mut self, gas_price: u128) {
        self.0.gas_price = gas_price;
    }
}

impl IntoTxEnv<ArbTransaction<TxEnv>> for ArbTransaction<TxEnv> {
    fn into_tx_env(self) -> ArbTransaction<TxEnv> {
        self
    }
}

impl revm::context_interface::Transaction for ArbTransaction<TxEnv> {
    type AccessListItem<'a> = <TxEnv as revm::context_interface::Transaction>::AccessListItem<'a> where Self: 'a;
    type Authorization<'a> = <TxEnv as revm::context_interface::Transaction>::Authorization<'a> where Self: 'a;

    fn tx_type(&self) -> u8 {
        revm::context_interface::Transaction::tx_type(&self.0)
    }
    fn caller(&self) -> alloy_primitives::Address {
        revm::context_interface::Transaction::caller(&self.0)
    }
    fn gas_limit(&self) -> u64 {
        revm::context_interface::Transaction::gas_limit(&self.0)
    }
    fn value(&self) -> alloy_primitives::U256 {
        revm::context_interface::Transaction::value(&self.0)
    }
    fn input(&self) -> &alloy_primitives::Bytes {
        revm::context_interface::Transaction::input(&self.0)
    }
    fn nonce(&self) -> u64 {
        revm::context_interface::Transaction::nonce(&self.0)
    }
    fn kind(&self) -> alloy_primitives::TxKind {
        revm::context_interface::Transaction::kind(&self.0)
    }
    fn chain_id(&self) -> Option<u64> {
        revm::context_interface::Transaction::chain_id(&self.0)
    }
    fn gas_price(&self) -> u128 {
        revm::context_interface::Transaction::gas_price(&self.0)
    }
    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        revm::context_interface::Transaction::access_list(&self.0)
    }
    fn blob_versioned_hashes(&self) -> &[alloy_primitives::B256] {
        revm::context_interface::Transaction::blob_versioned_hashes(&self.0)
    }
    fn max_fee_per_blob_gas(&self) -> u128 {
        revm::context_interface::Transaction::max_fee_per_blob_gas(&self.0)
    }
    fn authorization_list_len(&self) -> usize {
        revm::context_interface::Transaction::authorization_list_len(&self.0)
    }
    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        revm::context_interface::Transaction::authorization_list(&self.0)
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        revm::context_interface::Transaction::max_priority_fee_per_gas(&self.0)
    }
}

pub struct ArbEvm<DB: alloy_evm::Database + core::fmt::Debug, I> {
    inner: alloy_evm::EthEvm<DB, I, PrecompilesMap>,
}

impl<DB, I> ArbEvm<DB, I>
where
    DB: alloy_evm::Database + core::fmt::Debug,
{
    pub fn new(inner: alloy_evm::EthEvm<DB, I, PrecompilesMap>) -> Self {
        Self { inner }
    }
    pub fn into_inner(self) -> alloy_evm::EthEvm<DB, I, PrecompilesMap> {
        self.inner
    }
}

impl<DB, I> alloy_evm::Evm for ArbEvm<DB, I>
where
    DB: alloy_evm::Database + core::fmt::Debug,
    I: revm::inspector::Inspector<EthEvmContext<DB>>,
{
    type DB = DB;
    type Tx = ArbTransaction<TxEnv>;
    type Error = EVMError<<DB as revm::Database>::Error>;
    type HaltReason = revm::context_interface::result::HaltReason;
    type Spec = revm::primitives::hardfork::SpecId;
    type Precompiles = PrecompilesMap;
    type Inspector = I;

    fn block(&self) -> &revm::context::BlockEnv {
        self.inner.block()
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<revm::context_interface::result::ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_raw(tx.0)
    }

    fn transact_system_call(
        &mut self,
        caller: alloy_primitives::Address,
        contract: alloy_primitives::Address,
        data: alloy_primitives::Bytes,
    ) -> Result<revm::context_interface::result::ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, alloy_evm::EvmEnv<Self::Spec>) {
        self.inner.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inner.set_inspector_enabled(enabled)
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.inner.components()
    }

    fn components_mut(
        &mut self,
    ) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.inner.components_mut()
    }
}
pub trait ArbEvmExt {
    fn cfg_mut(&mut self) -> &mut revm::context::CfgEnv<revm::primitives::hardfork::SpecId>;
}
#[derive(Default, Debug, Clone, Copy)]
pub struct ArbEvmFactory(pub alloy_evm::EthEvmFactory);

impl ArbEvmFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EvmFactory for ArbEvmFactory {
    type Evm<DB: Database, I: revm::inspector::Inspector<EthEvmContext<DB>>> =
        ArbEvm<DB, I>;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Tx = ArbTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = revm::context_interface::result::HaltReason;
    type Spec = revm::primitives::hardfork::SpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
    ) -> Self::Evm<DB, NoOpInspector> {
        ArbEvm::new(self.0.create_evm(db, input))
    }

    fn create_evm_with_inspector<DB: Database, I: revm::inspector::Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        ArbEvm::new(self.0.create_evm_with_inspector(db, input, inspector))
    }
}
impl<DB, I> ArbEvmExt for ArbEvm<DB, I>
where
    DB: alloy_evm::Database + core::fmt::Debug,
    I: revm::inspector::Inspector<EthEvmContext<DB>>,
{
    fn cfg_mut(&mut self) -> &mut revm::context::CfgEnv<revm::primitives::hardfork::SpecId> {
        &mut self.inner.cfg
    }
}
