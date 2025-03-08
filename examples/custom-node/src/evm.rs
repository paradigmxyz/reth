use std::sync::Arc;

use crate::{chainspec::CustomChainSpec, primitives::CustomHeader};
use alloy_primitives::Address;
use reth_evm::{ConfigureEvm, EvmEnv};
use reth_node_api::ConfigureEvmEnv;
use reth_optimism_node::OpEvmConfig;

#[derive(Debug, Clone)]
pub struct CustomEvmConfig {
    inner: OpEvmConfig<CustomChainSpec>,
}

impl CustomEvmConfig {
    pub fn new(chain_spec: Arc<CustomChainSpec>) -> Self {
        Self { inner: OpEvmConfig::new(chain_spec) }
    }
}

impl ConfigureEvmEnv for CustomEvmConfig {
    type Header = CustomHeader;
    type Transaction = <OpEvmConfig as ConfigureEvmEnv>::Transaction;
    type Error = <OpEvmConfig as ConfigureEvmEnv>::Error;
    type TxEnv = <OpEvmConfig as ConfigureEvmEnv>::TxEnv;
    type Spec = <OpEvmConfig as ConfigureEvmEnv>::Spec;

    fn tx_env(&self, transaction: &Self::Transaction, signer: Address) -> Self::TxEnv {
        self.inner.tx_env(transaction, signer)
    }

    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: reth_node_api::NextBlockEnvAttributes,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }
}

impl ConfigureEvm for CustomEvmConfig {
    type Evm<'a, DB: reth_evm::Database + 'a, I: 'a> =
        <OpEvmConfig as ConfigureEvm>::Evm<'a, DB, I>;
    type EvmError<DBError: core::error::Error + Send + Sync + 'static> =
        <OpEvmConfig as ConfigureEvm>::EvmError<DBError>;
    type HaltReason = <OpEvmConfig as ConfigureEvm>::HaltReason;

    fn evm_with_env<DB: reth_evm::Database>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
    ) -> Self::Evm<'_, DB, ()> {
        self.inner.evm_with_env(db, evm_env)
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> Self::Evm<'_, DB, I>
    where
        DB: reth_evm::Database,
        I: revm::GetInspector<DB>,
    {
        self.inner.evm_with_env_and_inspector(db, evm_env, inspector)
    }
}
