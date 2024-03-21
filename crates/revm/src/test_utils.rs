use reth_interfaces::provider::ProviderResult;
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    keccak256, revm::config::revm_spec, trie::AccountProof, Account, Address, BlockNumber,
    Bytecode, Bytes, ChainSpec, Head, Header, StorageKey, Transaction, B256, U256,
};

#[cfg(not(feature = "optimism"))]
use reth_primitives::revm::env::fill_tx_env;
use reth_provider::{AccountReader, BlockHashReader, StateProvider, StateRootProvider};
use reth_trie::updates::TrieUpdates;
use revm::{
    db::BundleState,
    primitives::{AnalysisKind, CfgEnvWithHandlerCfg, TxEnv},
};
use std::collections::HashMap;
#[cfg(feature = "optimism")]
use {
    reth_primitives::revm::env::fill_op_tx_env,
    revm::{
        primitives::{HandlerCfg, SpecId},
        Database, Evm, EvmBuilder,
    },
};

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct StateProviderTest {
    accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
    contracts: HashMap<B256, Bytecode>,
    block_hash: HashMap<u64, B256>,
}

impl StateProviderTest {
    /// Insert account.
    pub fn insert_account(
        &mut self,
        address: Address,
        mut account: Account,
        bytecode: Option<Bytes>,
        storage: HashMap<StorageKey, U256>,
    ) {
        if let Some(bytecode) = bytecode {
            let hash = keccak256(&bytecode);
            account.bytecode_hash = Some(hash);
            self.contracts.insert(hash, Bytecode::new_raw(bytecode));
        }
        self.accounts.insert(address, (storage, account));
    }
}

impl AccountReader for StateProviderTest {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.get(&address).map(|(_, acc)| *acc))
    }
}

impl BlockHashReader for StateProviderTest {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        Ok(self.block_hash.get(&number).cloned())
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let range = start..end;
        Ok(self
            .block_hash
            .iter()
            .filter_map(|(block, hash)| range.contains(block).then_some(*hash))
            .collect())
    }
}

impl StateRootProvider for StateProviderTest {
    fn state_root(&self, _bundle_state: &BundleState) -> ProviderResult<B256> {
        unimplemented!("state root computation is not supported")
    }

    fn state_root_with_updates(
        &self,
        _bundle_state: &BundleState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!("state root computation is not supported")
    }
}

impl StateProvider for StateProviderTest {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<reth_primitives::StorageValue>> {
        Ok(self.accounts.get(&account).and_then(|(storage, _)| storage.get(&storage_key).cloned()))
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        Ok(self.contracts.get(&code_hash).cloned())
    }

    fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
        unimplemented!("proof generation is not supported")
    }
}

/// Test EVM configuration.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct TestEvmConfig;

impl ConfigureEvmEnv for TestEvmConfig {
    #[cfg(not(feature = "optimism"))]
    type TxMeta = ();
    #[cfg(feature = "optimism")]
    type TxMeta = Bytes;

    #[allow(unused_variables)]
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>,
    {
        #[cfg(not(feature = "optimism"))]
        fill_tx_env(tx_env, transaction, sender);
        #[cfg(feature = "optimism")]
        fill_op_tx_env(tx_env, transaction, sender, meta);
    }

    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        let spec_id = revm_spec(
            chain_spec,
            Head {
                number: header.number,
                timestamp: header.timestamp,
                difficulty: header.difficulty,
                total_difficulty,
                hash: Default::default(),
            },
        );

        cfg_env.chain_id = chain_spec.chain().id();
        cfg_env.perf_analyse_created_bytecodes = AnalysisKind::Analyse;

        cfg_env.handler_cfg.spec_id = spec_id;
        #[cfg(feature = "optimism")]
        {
            cfg_env.handler_cfg.is_optimism = chain_spec.is_optimism();
        }
    }
}

impl ConfigureEvm for TestEvmConfig {
    #[cfg(feature = "optimism")]
    fn evm<'a, DB: Database + 'a>(&self, db: DB) -> Evm<'a, (), DB> {
        let handler_cfg = HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true };
        EvmBuilder::default().with_db(db).with_handler_cfg(handler_cfg).build()
    }

    #[cfg(feature = "optimism")]
    fn evm_with_inspector<'a, DB: Database + 'a, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB> {
        let handler_cfg = HandlerCfg { spec_id: SpecId::LATEST, is_optimism: true };
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .with_handler_cfg(handler_cfg)
            .build()
    }
}
